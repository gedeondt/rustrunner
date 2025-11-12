use std::io::{Read as _, Write};

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

const SERVICE_NAME: &str = "sap-debt-adapter";
const PORT: u16 = 15003;
const CUSTOMER_UPDATE_QUEUE_ENDPOINT: &str = "queues/cliente-actualizado";

fn main() {
    Builder::from_env(Env::default().default_filter_or("info"))
        .format(|buf, record| writeln!(buf, "[{}] {}", record.level(), record.args()))
        .target(Target::Stdout)
        .init();

    let server = Server::http(("0.0.0.0", PORT)).expect("failed to bind facturación sap adapter service");
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
    if request.method() == &Method::Post {
        return handle_post(request);
    }

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

fn handle_post(request: Request) -> Result<(), Box<dyn std::error::Error>> {
    let full_path = request.url().to_owned();
    let path = full_path
        .split_once('?')
        .map(|(left, _)| left)
        .unwrap_or(full_path.as_str())
        .trim_start_matches('/');

    if path == CUSTOMER_UPDATE_QUEUE_ENDPOINT {
        handle_customer_update_event(request)?;
        return Ok(());
    }

    warn!("Rejecting POST to unknown path /{}", path);
    let response = Response::from_string("not found").with_status_code(404);
    request.respond(response)?;
    Ok(())
}

fn handle_customer_update_event(mut request: Request) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = String::new();
    if let Err(error) = request.as_reader().read_to_string(&mut payload) {
        warn!("Failed to read customer update event payload: {}", error);
    } else if !payload.trim().is_empty() {
        info!("Received customer update payload: {}", payload);
    }

    error!("Error actualizanando cliente");

    let mut response = Response::from_string("event processed").with_status_code(202);
    if let Ok(header) = Header::from_bytes(b"Content-Type", b"text/plain; charset=utf-8") {
        response = response.with_header(header);
    }
    request.respond(response)?;
    Ok(())
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
    fn dispatch_endpoint_returns_debt() {
        let response = dispatch_endpoint(&["sap", "customers", "123", "debt"]).expect("debt endpoint");
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert!(response.body.contains("\"customerId\":\"123\""));
    }
}
