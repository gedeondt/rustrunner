use std::convert::Infallible;
use std::env;
use std::io::Write;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use env_logger::{Builder, Env, Target};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use log::{error, info, warn};
use serde_json::json;

#[derive(Debug, PartialEq, Eq)]
struct EndpointResponse {
    status: u16,
    body: String,
    content_type: &'static str,
}

const SERVICE_NAME: &str = "cuenta-cliente-business";
const PORT: u16 = 15002;
const DEFAULT_RUNNER_BASE_URL: &str = "http://127.0.0.1:14000";
const CUSTOMER_UPDATE_QUEUE: &str = "clientes.actualizado";
const CUSTOMER_UPDATE_EVENT: &str = "ClienteActualizado";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    Builder::from_env(Env::default().default_filter_or("info"))
        .format(|buf, record| writeln!(buf, "[{}] {}", record.level(), record.args()))
        .target(Target::Stdout)
        .init();

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), PORT);
    info!(
        "Service '{SERVICE_NAME}' listening on http://{}:{}",
        "0.0.0.0",
        PORT
    );

    if let Err(error) = run_http_server(bind_addr).await {
        error!("HTTP server exited with error: {error}");
    }
}

async fn run_http_server(bind_addr: SocketAddr) -> Result<(), hyper::Error> {
    let make_service = make_service_fn(|_| async { Ok::<_, Infallible>(service_fn(handle_request)) });
    Server::bind(&bind_addr).serve(make_service).await
}

type EndpointResult = Response<Body>;

async fn handle_request(request: Request<Body>) -> Result<EndpointResult, Infallible> {
    if request.method() != Method::GET {
        warn!(
            "Rejecting request with unsupported method {:?} to {}",
            request.method(),
            request.uri()
        );
        return Ok(text_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"));
    }

    let segments: Vec<&str> = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    let Some(result) = dispatch_endpoint(&segments) else {
        return Ok(text_response(StatusCode::NOT_FOUND, "not found"));
    };

    Ok(endpoint_response(result))
}

fn endpoint_response(result: EndpointResponse) -> EndpointResult {
    let status = StatusCode::from_u16(result.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = Response::builder()
        .status(status)
        .body(Body::from(result.body))
        .expect("failed to build response");
    if let Ok(value) = HeaderValue::from_str(result.content_type) {
        response.headers_mut().insert(CONTENT_TYPE, value);
    }
    response
}

fn text_response(status: StatusCode, body: &str) -> EndpointResult {
    let mut response = Response::builder()
        .status(status)
        .body(Body::from(body.to_string()))
        .expect("failed to build text response");
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    response
}

fn dispatch_endpoint(segments: &[&str]) -> Option<EndpointResponse> {
    match segments {
        ["health"] => Some(EndpointResponse {
            status: 200,
            body: "ok".into(),
            content_type: "text/plain; charset=utf-8",
        }),
        ["accounts", account_id] => {
            let payload = json!({
                "accountId": account_id,
                "status": "active",
                "currency": "EUR",
                "balance": 18230.77,
                "available": 17650.12,
                "openedAt": "2015-03-12"
            });
            Some(json_response(payload))
        }
        ["accounts", account_id, "holders"] => {
            let payload = json!({
                "accountId": account_id,
                "holders": [
                    { "id": "cli-128", "name": "Laura Martínez", "role": "titular" },
                    { "id": "cli-745", "name": "Javier Ortiz", "role": "cotitular" }
                ]
            });
            Some(json_response(payload))
        }
        ["accounts", account_id, "limits"] => {
            let payload = json!({
                "accountId": account_id,
                "dailyTransferLimit": 5000,
                "remainingToday": 3200,
                "atmWithdrawalLimit": 1000,
                "lastReview": "2024-09-01"
            });
            Some(json_response(payload))
        }
        ["accounts", account_id, "movements"] => {
            let payload = json!({
                "accountId": account_id,
                "movements": [
                    {
                        "id": "mov-901",
                        "type": "transfer",
                        "amount": -350.25,
                        "beneficiary": "Comunidad de Propietarios",
                        "executedAt": "2024-11-19T09:12:00Z"
                    },
                    {
                        "id": "mov-902",
                        "type": "deposit",
                        "amount": 2450.00,
                        "origin": "Nómina",
                        "executedAt": "2024-11-15T07:05:00Z"
                    }
                ]
            });
            Some(json_response(payload))
        }
        ["webhooks", "customer-update"] => Some(trigger_customer_update_webhook()),
        _ => None,
    }
}

fn json_response(payload: serde_json::Value) -> EndpointResponse {
    EndpointResponse {
        status: 200,
        body: payload.to_string(),
        content_type: "application/json",
    }
}

fn trigger_customer_update_webhook() -> EndpointResponse {
    let publish_url = build_runner_publish_url();

    match publish_customer_update_event(&publish_url) {
        Ok(status) => {
            info!(
                "Published '{}' event to queue '{}' via {} (HTTP {status})",
                CUSTOMER_UPDATE_EVENT,
                CUSTOMER_UPDATE_QUEUE,
                publish_url
            );

            EndpointResponse {
                status: 202,
                body: json!({
                    "status": "queued",
                    "event": CUSTOMER_UPDATE_EVENT,
                    "queue": CUSTOMER_UPDATE_QUEUE
                })
                .to_string(),
                content_type: "application/json",
            }
        }
        Err(error) => {
            error!(
                "Failed to publish '{}' event to queue '{}': {error}",
                CUSTOMER_UPDATE_EVENT,
                CUSTOMER_UPDATE_QUEUE
            );

            EndpointResponse {
                status: 500,
                body: json!({
                    "status": "error",
                    "message": error
                })
                .to_string(),
                content_type: "application/json",
            }
        }
    }
}

fn build_runner_publish_url() -> String {
    let runner_base_url = env::var("RUNNER_BASE_URL").unwrap_or_else(|_| DEFAULT_RUNNER_BASE_URL.to_string());
    format!(
        "{}/__runner__/queues/{}",
        runner_base_url.trim_end_matches('/'),
        CUSTOMER_UPDATE_QUEUE
    )
}

fn publish_customer_update_event(publish_url: &str) -> Result<u16, String> {
    let payload = json!({
        "event": CUSTOMER_UPDATE_EVENT,
        "queue": CUSTOMER_UPDATE_QUEUE,
        "source": SERVICE_NAME,
    })
    .to_string();

    match ureq::post(publish_url)
        .set("Content-Type", "application/json")
        .send_string(&payload)
    {
        Ok(response) => {
            let status = response.status();
            let _ = response.into_string();
            Ok(status)
        }
        Err(ureq::Error::Status(status, response)) => {
            let _ = response.into_string();
            Err(format!("runner responded with HTTP {status}"))
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_endpoint_returns_account() {
        let response = dispatch_endpoint(&["accounts", "ES123" ]).expect("account endpoint");
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert!(response.body.contains("\"accountId\":\"ES123\""));
    }

    #[test]
    fn dispatch_endpoint_returns_health() {
        let response = dispatch_endpoint(&["health"]).expect("health endpoint");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");
    }
}
