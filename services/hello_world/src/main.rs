use tiny_http::{Method, Request, Response, Server};

const IDENTITY: &str = "hello";
const PORT: u16 = 15001;

fn main() {
    let server = Server::http(("0.0.0.0", PORT)).expect("failed to bind hello_world service");
    println!(
        "Service '{IDENTITY}' listening on http://{}:{}",
        "0.0.0.0",
        PORT
    );

    for request in server.incoming_requests() {
        if let Err(error) = handle_request(request) {
            eprintln!("failed to handle request: {error}");
        }
    }
}

fn handle_request(request: Request) -> Result<(), Box<dyn std::error::Error>> {
    if request.method() != &Method::Get {
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

    let response = match endpoint {
        "hello" => Response::from_string(format!("Soy {IDENTITY} y digo hello"))
            .with_status_code(200),
        "bye" => Response::from_string(format!("Soy {IDENTITY} y digo bye")).with_status_code(200),
        "health" => Response::from_string("ok").with_status_code(200),
        _ => {
            let response = Response::from_string("not found").with_status_code(404);
            request.respond(response)?;
            return Ok(());
        }
    };

    request.respond(response)?;
    Ok(())
}
