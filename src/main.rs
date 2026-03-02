use actix_web::{delete, get, post, web, App, HttpResponse, HttpServer, Responder};
use actix_web_prom::PrometheusMetricsBuilder;
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::env;
use tera::{Context, Tera};

#[derive(Debug, Serialize, sqlx::FromRow)]
struct Link {
    code: String,
    target_url: String,
    created_at: DateTime<Utc>,
    hit_count: i64,
}

#[derive(Debug, Deserialize)]
struct CreateLinkRequest {
    url: String,
    code: Option<String>,
}

struct AppState {
    db: PgPool,
    tera: Tera,
}

fn random_code() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

async fn fetch_all_links(db: &PgPool) -> Vec<Link> {
    sqlx::query_as::<_, Link>(
        "SELECT code, target_url, created_at, hit_count FROM links ORDER BY created_at DESC",
    )
    .fetch_all(db)
    .await
    .unwrap_or_default()
}

fn render_page(tera: &Tera, links: &[Link]) -> HttpResponse {
    let mut ctx = Context::new();
    ctx.insert("links", links);
    match tera.render("index.html", &ctx) {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(e) => HttpResponse::InternalServerError().body(format!("Template error: {}", e)),
    }
}

#[get("/health")]
async fn health() -> impl Responder {
    HttpResponse::Ok().body("OK")
}

#[get("/")]
async fn index(data: web::Data<AppState>) -> impl Responder {
    let links = fetch_all_links(&data.db).await;
    render_page(&data.tera, &links)
}

#[post("/links")]
async fn create_link(
    data: web::Data<AppState>,
    form: web::Json<CreateLinkRequest>,
) -> impl Responder {
    let code = match &form.code {
        Some(c) if !c.is_empty() => c.clone(),
        _ => random_code(),
    };

    let url = form.url.trim().to_string();
    if url.is_empty() {
        return HttpResponse::BadRequest().body("URL is required");
    }

    let result = sqlx::query(
        "INSERT INTO links (code, target_url) VALUES ($1, $2) ON CONFLICT (code) DO NOTHING",
    )
    .bind(&code)
    .bind(&url)
    .execute(&data.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            let links = fetch_all_links(&data.db).await;
            render_page(&data.tera, &links)
        }
        Ok(_) => HttpResponse::Conflict().body(format!("Code '{}' already exists", code)),
        Err(e) => HttpResponse::InternalServerError().body(format!("Database error: {}", e)),
    }
}

#[get("/{code}")]
async fn redirect(data: web::Data<AppState>, path: web::Path<String>) -> impl Responder {
    let code = path.into_inner();

    // Skip special routes handled by other handlers
    if code == "health" || code == "metrics" || code == "links" {
        return HttpResponse::NotFound().body("Not found");
    }

    let result = sqlx::query_scalar::<_, String>(
        "UPDATE links SET hit_count = hit_count + 1 WHERE code = $1 RETURNING target_url",
    )
    .bind(&code)
    .fetch_optional(&data.db)
    .await;

    match result {
        Ok(Some(target_url)) => HttpResponse::MovedPermanently()
            .append_header(("Location", target_url))
            .finish(),
        Ok(None) => HttpResponse::NotFound().body("Short link not found"),
        Err(e) => HttpResponse::InternalServerError().body(format!("Database error: {}", e)),
    }
}

#[delete("/links/{code}")]
async fn delete_link(data: web::Data<AppState>, path: web::Path<String>) -> impl Responder {
    let code = path.into_inner();

    let result = sqlx::query("DELETE FROM links WHERE code = $1")
        .bind(&code)
        .execute(&data.db)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            let links = fetch_all_links(&data.db).await;
            render_page(&data.tera, &links)
        }
        Ok(_) => HttpResponse::NotFound().body("Link not found"),
        Err(e) => HttpResponse::InternalServerError().body(format!("Database error: {}", e)),
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv::dotenv().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let template_dir = env::var("TEMPLATE_DIR").unwrap_or_else(|_| "templates".to_string());

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to database");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS links (
            code TEXT PRIMARY KEY,
            target_url TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            hit_count BIGINT NOT NULL DEFAULT 0
        )",
    )
    .execute(&pool)
    .await
    .expect("Failed to run migrations");

    let tera = Tera::new(&format!("{}/**/*", template_dir)).expect("Failed to load templates");

    let prometheus = PrometheusMetricsBuilder::new("shortlink")
        .endpoint("/metrics")
        .build()
        .expect("Failed to build prometheus metrics");

    let data = web::Data::new(AppState { db: pool, tera });

    println!("Starting shortlink on port {}", port);

    HttpServer::new(move || {
        App::new()
            .wrap(prometheus.clone())
            .app_data(data.clone())
            .service(health)
            .service(index)
            .service(create_link)
            .service(delete_link)
            .service(redirect)
    })
    .bind(format!("0.0.0.0:{}", port))?
    .run()
    .await
}
