use std::convert::Infallible;
use std::io::Write;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use env_logger::{Builder, Env, Target};
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

const SERVICE_NAME: &str = "customer-bff";
const PORT: u16 = 15001;

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

fn dispatch_endpoint(segments: &[&str]) -> Option<EndpointResponse> {
    match segments {
        ["health"] => Some(EndpointResponse {
            status: 200,
            body: "ok".into(),
            content_type: "text/plain; charset=utf-8",
        }),
        ["clients", client_id, "summary"] => {
            let payload = json!({
                "clientId": client_id,
                "name": "Laura Martínez",
                "segment": "premium",
                "lastLogin": "2024-11-18T08:45:00Z",
                "preferredProducts": ["tarjeta_black", "cuenta_remunerada"],
            });
            Some(json_response(payload))
        }
        ["clients", client_id, "products"] => {
            let payload = json!({
                "clientId": client_id,
                "products": [
                    {
                        "type": "account",
                        "alias": "Cuenta Nómina",
                        "balance": 12890.43,
                        "currency": "EUR"
                    },
                    {
                        "type": "creditCard",
                        "alias": "Visa Infinite",
                        "creditLine": 12000,
                        "available": 8300
                    },
                    {
                        "type": "loan",
                        "alias": "Préstamo coche",
                        "outstanding": 8200,
                        "nextPayment": "2024-12-05"
                    }
                ]
            });
            Some(json_response(payload))
        }
        ["clients", client_id, "alerts"] => {
            let payload = json!({
                "clientId": client_id,
                "alerts": [
                    {
                        "id": "alert-001",
                        "type": "security",
                        "message": "Inicio de sesión desde un dispositivo nuevo",
                        "createdAt": "2024-11-20T06:10:00Z"
                    },
                    {
                        "id": "alert-014",
                        "type": "product",
                        "message": "Tu tarjeta Visa Infinite tiene una cuota pendiente",
                        "createdAt": "2024-11-15T09:30:00Z"
                    }
                ]
            });
            Some(json_response(payload))
        }
        ["clients", client_id, "contact"] => {
            let payload = json!({
                "clientId": client_id,
                "assignedManager": {
                    "name": "Andrea Gómez",
                    "email": "andrea.gomez@bankia.example",
                    "phone": "+34 912 000 123"
                },
                "preferredBranch": {
                    "id": "BR-042",
                    "name": "Oficina Castellana",
                    "address": "Paseo de la Castellana 45, Madrid"
                }
            });
            Some(json_response(payload))
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_endpoint_returns_health() {
        let response = dispatch_endpoint(&["health"]).expect("health endpoint");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");
        assert_eq!(response.content_type, "text/plain; charset=utf-8");
    }

    #[test]
    fn dispatch_endpoint_returns_summary() {
        let response = dispatch_endpoint(&["clients", "123", "summary"]).expect("summary endpoint");
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert!(response.body.contains("\"clientId\":\"123\""));
    }
}
