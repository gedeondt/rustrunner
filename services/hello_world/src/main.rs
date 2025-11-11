use std::io::Write;

use env_logger::{Builder, Env, Target};
use log::{debug, error, info, warn};
use serde_json::json;
use tiny_http::{Method, Request, Response, Server};
use ureq::Error;

#[derive(Debug, PartialEq, Eq)]
struct EndpointResponse {
    status: u16,
    body: String,
}

const IDENTITY: &str = "hello";
const PORT: u16 = 15001;
const QUEUE_NAME: &str = "hello.notifications";
const RUNNER_QUEUE_ENDPOINT: &str = "http://127.0.0.1:14000/__runner__/queues/hello.notifications";

fn main() {
    Builder::from_env(Env::default().default_filter_or("info"))
        .format(|buf, record| writeln!(buf, "[{}] {}", record.level(), record.args()))
        .target(Target::Stdout)
        .init();

    let server = Server::http(("0.0.0.0", PORT)).expect("failed to bind hello_world service");
    info!(
        "Service '{IDENTITY}' listening on http://{}:{}",
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

    let (path, _) = request.url().split_once('?').unwrap_or((request.url(), ""));
    let mut segments = path
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty());

    let Some(endpoint) = segments.next() else {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    };

    if segments.next().is_some() {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    debug!("Dispatching endpoint '{}'", endpoint);

    match dispatch_endpoint(endpoint) {
        Some(result) => {
            info!("Procesando endpoint /{}", endpoint);
            request
                .respond(Response::from_string(result.body).with_status_code(result.status))?;
        }
        None => {
            warn!("Endpoint desconocido '{}'", endpoint);
            let response = Response::from_string("not found").with_status_code(404);
            request.respond(response)?;
        }
    }

    Ok(())
}

fn dispatch_endpoint(endpoint: &str) -> Option<EndpointResponse> {
    match endpoint {
        "hello" => Some(EndpointResponse {
            status: 200,
            body: format!("Soy {IDENTITY} y digo hello"),
        }),
        "bye" => Some(EndpointResponse {
            status: 200,
            body: format!("Soy {IDENTITY} y digo bye"),
        }),
        "health" => Some(EndpointResponse {
            status: 200,
            body: "ok".into(),
        }),
        "notify" => Some(match publish_greeting_event() {
            Ok(message) => EndpointResponse {
                status: 202,
                body: message,
            },
            Err(error) => EndpointResponse {
                status: 500,
                body: error,
            },
        }),
        _ => None,
    }
}

fn publish_greeting_event() -> Result<String, String> {
    let payload = json!({
        "queue": QUEUE_NAME,
        "message": "Hola desde hello",
        "origin": IDENTITY,
    });

    match ureq::post(RUNNER_QUEUE_ENDPOINT)
        .set("Content-Type", "application/json")
        .send_string(&payload.to_string())
    {
        Ok(response) => {
            let status = response.status();
            let body = response
                .into_string()
                .unwrap_or_else(|_| String::from(""));

            if (200..300).contains(&status) {
                if body.is_empty() {
                    Ok("Evento publicado correctamente".into())
                } else {
                    Ok(format!("Evento publicado: {}", body))
                }
            } else {
                Err(format!("El runner devolvió un estado {}: {}", status, body))
            }
        }
        Err(Error::Status(status, response)) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| String::from(""));
            Err(format!(
                "El runner rechazó el evento ({}): {}",
                status,
                body
            ))
        }
        Err(error) => Err(format!("No se pudo publicar el evento: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_endpoint_returns_expected_payloads() {
        let hello = dispatch_endpoint("hello").expect("hello endpoint");
        assert_eq!(hello.status, 200);
        assert!(hello.body.contains("digo hello"));

        let health = dispatch_endpoint("health").expect("health endpoint");
        assert_eq!(health.status, 200);
        assert_eq!(health.body, "ok");

        assert!(dispatch_endpoint("unknown").is_none());
    }
}
