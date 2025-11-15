use std::convert::Infallible;
use std::io::Write;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use bytes::Bytes;
use env_logger::{Builder, Env, Target};
use hyper::body;
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use log::{error, info, warn};
use serde_json::json;

type HttpResponse = Response<Body>;
type HandlerResult = Result<HttpResponse, Infallible>;

#[derive(Debug, PartialEq, Eq)]
struct EndpointResponse {
    status: u16,
    body: String,
    content_type: &'static str,
}

const SERVICE_NAME: &str = "sap-debt-adapter";
const PORT: u16 = 15003;
const CUSTOMER_UPDATE_QUEUE_ENDPOINT: &str = "queues/cliente-actualizado";

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

async fn handle_request(request: Request<Body>) -> HandlerResult {
    if request.method() == Method::POST {
        return handle_post(request).await;
    }

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

fn dispatch_endpoint(segments: &[&str]) -> Option<EndpointResponse> {
    match segments {
        ["health"] => Some(EndpointResponse {
            status: 200,
            body: "ok".into(),
            content_type: "text/plain; charset=utf-8",
        }),
        ["sap", "customers", customer_id, "debt"] => {
            let payload = json!({
                "customerId": customer_id,
                "currency": "EUR",
                "totalOpen": 1890.55,
                "overdueAmount": 450.20,
                "oldestDueDate": "2024-08-30",
                "lastSynchronizedAt": "2024-11-20T04:30:00Z"
            });
            Some(json_response(payload))
        }
        ["sap", "customers", customer_id, "invoices"] => {
            let payload = json!({
                "customerId": customer_id,
                "invoices": [
                    {
                        "id": "INV-2024-001",
                        "status": "overdue",
                        "issuedAt": "2024-07-15",
                        "dueAt": "2024-08-30",
                        "amount": 450.20
                    },
                    {
                        "id": "INV-2024-017",
                        "status": "open",
                        "issuedAt": "2024-10-01",
                        "dueAt": "2024-12-01",
                        "amount": 980.00
                    }
                ]
            });
            Some(json_response(payload))
        }
        ["sap", "customers", customer_id, "status"] => {
            let payload = json!({
                "customerId": customer_id,
                "riskLevel": "medium",
                "paymentBehavior": "irregular",
                "blocked": false,
                "lastAssessment": "2024-11-01"
            });
            Some(json_response(payload))
        }
        ["sap", "customers", customer_id, "contacts"] => {
            let payload = json!({
                "customerId": customer_id,
                "contacts": [
                    {
                        "name": "Departamento de Facturación",
                        "email": "facturacion@sap.example",
                        "phone": "+34 913 555 210"
                    },
                    {
                        "name": "Gestión de Cobro",
                        "email": "cobros@sap.example",
                        "phone": "+34 913 555 220"
                    }
                ]
            });
            Some(json_response(payload))
        }
        _ => None,
    }
}

async fn handle_post(request: Request<Body>) -> HandlerResult {
    let path = request.uri().path().trim_start_matches('/');

    if path == CUSTOMER_UPDATE_QUEUE_ENDPOINT {
        return handle_customer_update_event(request).await;
    }

    warn!("Rejecting POST to unknown path /{}", path);
    Ok(text_response(StatusCode::NOT_FOUND, "not found"))
}

async fn handle_customer_update_event(request: Request<Body>) -> HandlerResult {
    let body = match body::to_bytes(request.into_body()).await {
        Ok(collected) => collected,
        Err(error) => {
            warn!("Failed to read customer update event payload: {error}");
            Bytes::new()
        }
    };

    if !body.is_empty() {
        let payload = String::from_utf8_lossy(&body);
        info!("Received customer update payload: {}", payload);
    }

    error!("Error actualizanando cliente");
    Ok(text_response(StatusCode::ACCEPTED, "event processed"))
}

fn json_response(payload: serde_json::Value) -> EndpointResponse {
    EndpointResponse {
        status: 200,
        body: payload.to_string(),
        content_type: "application/json",
    }
}

fn endpoint_response(result: EndpointResponse) -> HttpResponse {
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

fn text_response(status: StatusCode, body: &str) -> HttpResponse {
    let mut response = Response::builder()
        .status(status)
        .body(Body::from(body.to_string()))
        .expect("failed to build text response");
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_endpoint_returns_debt() {
        let response = dispatch_endpoint(&["sap", "customers", "123", "debt"]).expect("debt endpoint");
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert!(response.body.contains("\"customerId\":\"123\""));
    }
}
