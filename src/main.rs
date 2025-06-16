use axum::{
    Router,
    extract::{Query, State},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use lazy_static::lazy_static;
use regex::Regex;
use reqwest::header;
use scraper::{Html as ScraperHtml, Selector};
use serde::Deserialize;
use std::net::SocketAddr;
use url::Url;

// Use lazy_static to compile the regex once.
lazy_static! {
    static ref DOI_RE: Regex = Regex::new(r"^(?:https?://)?(?:dx\.)?doi\.org/(.+)").unwrap();
}

// --- Structs for Deserializing Metadata ---

// Represents the query parameter from the URL, e.g., /get_bibtex?url=...
#[derive(Deserialize)]
struct BibtexQuery {
    url: String,
}

// Structs for parsing Schema.org JSON-LD data.
#[derive(Deserialize, Debug)]
struct SchemaArticle {
    #[serde(rename = "@type")]
    type_of: String,
    headline: Option<String>,
    #[serde(default)]
    author: Vec<SchemaAuthor>,
    #[serde(rename = "datePublished")]
    date_published: Option<String>,
}

#[derive(Deserialize, Debug)]
struct SchemaAuthor {
    name: String,
}

#[derive(Deserialize, Debug)]
struct SchemaPublisher {
}

// --- Application State and Error Handling ---

// A simple struct to hold our reqwest client.
#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
}

// Custom error type for better error handling.
enum AppError {
    RequestError(reqwest::Error),
    UrlParseError(url::ParseError),
    ExtractionError(String),
}

// Implement IntoResponse for our custom error, so Axum can convert it into an HTTP response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            AppError::RequestError(err) => (
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to fetch the URL: {}", err),
            ),
            AppError::UrlParseError(err) => (
                reqwest::StatusCode::BAD_REQUEST,
                format!("Invalid URL provided: {}", err),
            ),
            AppError::ExtractionError(msg) => (
                reqwest::StatusCode::NOT_FOUND,
                format!("Could not extract BibTeX data: {}", msg),
            ),
        };
        (status, error_message).into_response()
    }
}

// --- Main Application Logic ---

#[tokio::main]
async fn main() {
    // Create a shared reqwest client.
    let shared_state = AppState {
        client: reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36")
            .build()
            .unwrap(),
    };

    // Build our application with two routes: one for the UI and one for the API.
    let app = Router::new()
        .route("/", get(show_form))
        .route("/get_bibtex", get(get_bibtex_handler))
        .with_state(shared_state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    println!("-> Listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Handler for the main page, showing a simple HTML form.
async fn show_form() -> Html<&'static str> {
    Html(
        r#"
        <!doctype html>
        <html>
            <head>
                <title>BibTeX Extractor</title>
                <style>
                    body { font-family: sans-serif; max-width: 800px; margin: auto; padding: 2em; background: #f4f4f4; }
                    input { width: 100%; padding: 8px; margin-bottom: 1em; }
                    pre { background: #e3e3e3; padding: 1em; white-space: pre-wrap; word-wrap: break-word; }
                </style>
            </head>
            <body>
                <h1>Rust BibTeX Extractor</h1>
                <p>Enter a URL to attempt to extract its BibTeX entry.</p>
                <form action="/get_bibtex" method="get">
                    <input type="url" name="url" placeholder="https://example.com" required>
                    <button type="submit">Get BibTeX</button>
                </form>
            </body>
        </html>
        "#,
    )
}

/// The main handler that drives the BibTeX extraction logic.
async fn get_bibtex_handler(
    State(state): State<AppState>,
    Query(query): Query<BibtexQuery>,
) -> Result<Html<String>, AppError> {
    let bibtex_entry = fetch_and_generate_bibtex(&state.client, &query.url).await?;

    // Format the output into a simple HTML response
    let html_response = format!(
        r#"
        <!doctype html>
        <html>
            <head>
                <title>BibTeX Result</title>
                <style>
                    body {{ font-family: sans-serif; max-width: 800px; margin: auto; padding: 2em; background: #f4f4f4; }}
                    pre {{ background: #e3e3e3; padding: 1em; white-space: pre-wrap; word-wrap: break-word; border: 1px solid #ccc; position: relative; }}
                    a {{ color: #007bff; }}
                    .copy-button {{
                        position: absolute;
                        top: 10px;
                        right: 10px;
                        padding: 8px 16px;
                        background: #007bff;
                        color: white;
                        border: none;
                        border-radius: 4px;
                        cursor: pointer;
                        font-size: 14px;
                    }}
                    .copy-button:hover {{
                        background: #0056b3;
                    }}
                    .copy-button.copied {{
                        background: #28a745;
                    }}
                </style>
            </head>
            <body>
                <h1>BibTeX Result</h1>
                <p>Source URL: <a href="{url}">{url}</a></p>
                <div style="position: relative;">
                    <pre><code id="bibtex-content">{entry}</code></pre>
                    <button class="copy-button" onclick="copyBibTeX()">Copy BibTeX</button>
                </div>
                <a href="/">Try another URL</a>

                <script>
                    function copyBibTeX() {{
                        const content = document.getElementById('bibtex-content').textContent;
                        navigator.clipboard.writeText(content).then(() => {{
                            const button = document.querySelector('.copy-button');
                            const originalText = button.textContent;
                            button.textContent = 'Copied!';
                            button.classList.add('copied');
                            setTimeout(() => {{
                                button.textContent = originalText;
                                button.classList.remove('copied');
                            }}, 2000);
                        }});
                    }}
                </script>
            </body>
        </html>
        "#,
        url = query.url,
        entry = html_escape::encode_text(&bibtex_entry)
    );

    Ok(Html(html_response))
}

/// Core logic: Fetches URL content and tries various methods to generate BibTeX.
async fn fetch_and_generate_bibtex(
    client: &reqwest::Client,
    url_str: &str,
) -> Result<String, AppError> {
    // --- Strategy 1: Check for DOI ---
    if let Some(caps) = DOI_RE.captures(url_str) {
        if let Some(doi) = caps.get(1) {
            let doi_url = format!("https://doi.org/{}", doi.as_str());
            let mut headers = header::HeaderMap::new();
            headers.insert(
                header::ACCEPT,
                "application/x-bibtex; charset=utf-8".parse().unwrap(),
            );

            let res = client
                .get(&doi_url)
                .headers(headers)
                .send()
                .await
                .map_err(AppError::RequestError)?;

            if res.status().is_success() {
                let text = res.text().await.map_err(AppError::RequestError)?;
                if !text.trim().is_empty() && text.starts_with('@') {
                    println!("-> Found BibTeX via DOI content negotiation.");
                    return Ok(text);
                }
            }
        }
    }

    // --- Strategy 2: Scrape the webpage for metadata ---
    println!("-> DOI method failed or not applicable. Falling back to HTML scraping.");
    let res = client
        .get(url_str)
        .send()
        .await
        .map_err(AppError::RequestError)?;

    if !res.status().is_success() {
        return Err(AppError::ExtractionError(format!(
            "URL returned status {}",
            res.status()
        )));
    }

    let html_content = res.text().await.map_err(AppError::RequestError)?;
    let document = ScraperHtml::parse_document(&html_content);

    // Use the parsed URL to get the hostname for the BibTeX entry.
    let parsed_url = Url::parse(url_str).map_err(AppError::UrlParseError)?;
    let site_name = parsed_url.host_str().unwrap_or_default();

    // --- Extract metadata in order of preference ---
    let (title, author, year) = extract_metadata(&document);

    if title.is_empty() {
        return Err(AppError::ExtractionError(
            "Could not find a title for the page.".into(),
        ));
    }

    // --- Assemble the BibTeX entry ---
    let citation_key = generate_citation_key(&author, &year, &title);

    let mut bibtex = String::from("@misc{");
    bibtex.push_str(&citation_key);
    bibtex.push_str(",\n");
    bibtex.push_str(&format!("  title = {{{}}},\n", title));
    if !author.is_empty() {
        bibtex.push_str(&format!("  author = {{{}}},\n", author));
    }
    bibtex.push_str(&format!("  howpublished = {{\\url{{{}}}}},\n", url_str));
    bibtex.push_str(&format!(
        "  note = {{Accessed: {}}},\n",
        chrono::Local::now().format("%Y-%m-%d")
    ));
    if !year.is_empty() {
        bibtex.push_str(&format!("  year = {{{}}},\n", year));
    }
    bibtex.push_str(&format!(
        "  urldate = {{{}}},\n",
        chrono::Local::now().format("%Y-%m-%d")
    ));
    bibtex.push_str(&format!("  publisher = {{{}}},\n", site_name));
    bibtex.push('}');

    Ok(bibtex)
}

/// Helper to extract metadata from a parsed HTML document.
fn extract_metadata(document: &ScraperHtml) -> (String, String, String) {
    // Strategy 2a: Look for Schema.org JSON-LD (best source)
    if let Some((title, author, year)) = extract_from_schema(document) {
        println!("-> Extracted metadata from Schema.org JSON-LD.");
        return (title, author, year);
    }

    // Strategy 2b: Look for OpenGraph and other meta tags
    let title = select_text(document, "meta[property='og:title']", "content")
        .or_else(|| select_text(document, "title", "text"))
        .unwrap_or_default();

    let author = select_text(document, "meta[name='author']", "content")
        .or_else(|| select_text(document, "meta[property='article:author']", "content"))
        .unwrap_or_default();

    let year = select_text(
        document,
        "meta[property='article:published_time']",
        "content",
    )
    .map(|s| s[..4].to_string()) // Take first 4 chars for year
    .unwrap_or_default();

    println!("-> Extracted metadata from meta tags.");
    (title, author, year)
}

/// Specific helper for extracting from Schema.org JSON-LD scripts.
fn extract_from_schema(document: &ScraperHtml) -> Option<(String, String, String)> {
    let selector = Selector::parse("script[type='application/ld+json']").unwrap();
    for element in document.select(&selector) {
        let json_text = element.inner_html();
        if let Ok(article) = serde_json::from_str::<SchemaArticle>(&json_text) {
            if &article.type_of == "Article"
                || &article.type_of == "NewsArticle"
                || &article.type_of == "BlogPosting"
            {
                let title = article.headline.unwrap_or_default();
                let authors = article
                    .author
                    .into_iter()
                    .map(|a| a.name)
                    .collect::<Vec<_>>()
                    .join(" and ");
                let year = article
                    .date_published
                    .map(|s| s[..4].to_string())
                    .unwrap_or_default();

                if !title.is_empty() {
                    return Some((title, authors, year));
                }
            }
        }
    }
    None
}

/// Generic helper to select text from an element attribute or inner text.
fn select_text(document: &ScraperHtml, selector_str: &str, attr: &str) -> Option<String> {
    let selector = Selector::parse(selector_str).ok()?;
    document.select(&selector).next().and_then(|element| {
        if attr == "text" {
            Some(element.inner_html().trim().to_string())
        } else {
            element.value().attr(attr).map(|s| s.trim().to_string())
        }
    })
}

/// Generates a simple BibTeX citation key like "Doe2025FirstWord".
fn generate_citation_key(author: &str, year: &str, title: &str) -> String {
    let author_part = author.split_whitespace().next().unwrap_or("Unknown");
    let year_part = if !year.is_empty() { year } else { "ND" }; // ND for No Date
    let title_part = title.split_whitespace().next().unwrap_or("NoTitle");

    format!(
        "{}{}{}",
        author_part
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>(),
        year_part,
        title_part
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    )
}
