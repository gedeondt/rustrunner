use std::io::Write;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use env_logger::{Builder, Env, Target};
use log::{error, info, warn};
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server};

#[derive(Debug, PartialEq, Eq)]
struct EndpointResponse {
    status: u16,
    body: String,
    content_type: &'static str,
}

const SERVICE_NAME: &str = "customer-bff";
const PORT: u16 = 15001;

fn main() {
    Builder::from_env(Env::default().default_filter_or("info"))
        .format(|buf, record| writeln!(buf, "[{}] {}", record.level(), record.args()))
        .target(Target::Stdout)
        .init();

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), PORT);
    let server = Server::http(bind_addr).expect("failed to bind atención cliente bff service");
    info!(
        "Service '{SERVICE_NAME}' listening on http://{}:{}",
        "0.0.0.0",
        PORT
    );

    for request in server.incoming_requests() {
        if let Err(error) = handle_request(request) {
            error!("failed to handle request: {error}");
        }
    }
}

fn handle_request(request: Request) -> Result<(), Box<dyn std::error::Error>> {
    if request.method() != &Method::Get {
        warn!(
            "Rejecting request with unsupported method {:?} to {}",
            request.method(),
            request.url()
        );
        let response = Response::from_string("method not allowed").with_status_code(405);
        request.respond(response)?;
        return Ok(());
    }

    let (path, _) = request
        .url()
        .split_once('?')
        .map(|(left, _)| (left, ()))
        .unwrap_or((request.url(), ()));

    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    let Some(result) = dispatch_endpoint(&segments) else {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    };

    let mut response = Response::from_string(result.body).with_status_code(result.status);
    if let Ok(header) = Header::from_bytes(b"Content-Type", result.content_type.as_bytes()) {
        response = response.with_header(header);
    }
    request.respond(response)?;
    Ok(())
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
