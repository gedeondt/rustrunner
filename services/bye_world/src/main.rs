use std::io::Write;

use env_logger::{Builder, Env, Target};
use log::{debug, error, info, warn};
use tiny_http::{Method, Request, Response, Server};

const IDENTITY: &str = "bye";
const PORT: u16 = 15002;
const HELLO_NOTIFICATIONS_QUEUE: &str = "hello.notifications";

#[derive(Debug, PartialEq, Eq)]
struct EndpointResponse {
    status: u16,
    body: String,
}

fn main() {
    Builder::from_env(Env::default().default_filter_or("info"))
        .format(|buf, record| writeln!(buf, "[{}] {}", record.level(), record.args()))
        .target(Target::Stdout)
        .init();

    let server = Server::http(("0.0.0.0", PORT)).expect("failed to bind bye_world service");
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
    match *request.method() {
        Method::Get => handle_get(request),
        Method::Post => handle_post(request),
        _ => {
            warn!(
                "Rejecting request with unsupported method {:?} to {}",
                request.method(),
                request.url()
            );
            let response = Response::from_string("method not allowed").with_status_code(405);
            request.respond(response)?;
            Ok(())
        }
    }
}

fn handle_get(request: Request) -> Result<(), Box<dyn std::error::Error>> {
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
            request.respond(Response::from_string(result.body).with_status_code(result.status))?;
        }
        None => {
            warn!("Endpoint desconocido '{}'", endpoint);
            let response = Response::from_string("not found").with_status_code(404);
            request.respond(response)?;
        }
    }

    Ok(())
}

fn handle_post(mut request: Request) -> Result<(), Box<dyn std::error::Error>> {
    let (path, _) = request.url().split_once('?').unwrap_or((request.url(), ""));
    let normalized = path.trim_start_matches('/');

    if normalized != "events/hello" {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    let mut body = String::new();
    if let Err(error) = request.as_reader().read_to_string(&mut body) {
        error!("No se pudo leer el payload del evento: {}", error);
        let response = Response::from_string("invalid payload").with_status_code(500);
        request.respond(response)?;
        return Ok(());
    }

    let queue_name = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("X-Rustrunner-Queue"))
        .map(|header| header.value.as_str())
        .unwrap_or(HELLO_NOTIFICATIONS_QUEUE);

    info!(
        "Evento recibido en la cola '{}' con payload: {}",
        queue_name,
        body
    );

    let response = Response::from_string("accepted").with_status_code(202);
    request.respond(response)?;
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
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_endpoint_returns_expected_payloads() {
        let bye = dispatch_endpoint("bye").expect("bye endpoint");
        assert_eq!(bye.status, 200);
        assert!(bye.body.contains("digo bye"));

        let health = dispatch_endpoint("health").expect("health endpoint");
        assert_eq!(health.status, 200);
        assert_eq!(health.body, "ok");

        assert!(dispatch_endpoint("unknown").is_none());
    }
}
