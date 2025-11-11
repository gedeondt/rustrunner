use anyhow::{anyhow, Result};
use std::io::Cursor;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::config::Service;
use crate::health::{HealthStatus, SharedHealthMap};
use crate::logs::SharedLogMap;

const ENTRY_PORT: u16 = 14000;

pub fn run_server(
    services: &[Service],
    health: &SharedHealthMap,
    logs: &SharedLogMap,
) -> Result<()> {
    let server = Server::http(("0.0.0.0", ENTRY_PORT)).map_err(|error| {
        anyhow!(
            "failed to bind entrypoint to port {}: {}",
            ENTRY_PORT,
            error
        )
    })?;

    println!("Runner listening on http://{}:{}", "0.0.0.0", ENTRY_PORT);

    for request in server.incoming_requests() {
        if let Err(error) = handle_request(services, health, logs, request) {
            eprintln!("Failed to handle request: {:#}", error);
        }
    }

    Ok(())
}

fn handle_request(
    services: &[Service],
    health: &SharedHealthMap,
    logs: &SharedLogMap,
    request: Request,
) -> Result<()> {
    if request.method() != &Method::Get {
        let response = Response::from_string("method not allowed").with_status_code(405);
        request.respond(response)?;
        return Ok(());
    }

    let full_path = request.url().to_owned();
    let (path, query) = match full_path.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (full_path.as_str(), None),
    };

    let trimmed_path = path.trim_start_matches('/');

    if trimmed_path == "health" {
        let response = Response::from_string("ok").with_status_code(200);
        request.respond(response)?;
        return Ok(());
    }

    if trimmed_path.is_empty() {
        let response = render_homepage(services, health);
        request.respond(response)?;
        return Ok(());
    }

    if let Some(rest) = trimmed_path.strip_prefix("__runner__/services/") {
        return handle_internal_service_request(services, logs, request, rest);
    }

    let Some((service, endpoint_path)) = resolve_service_route(services, trimmed_path) else {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    };

    if !service.supports(request.method(), &endpoint_path) {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    let mut target_url = format!(
        "{}/{}",
        service.base_url.trim_end_matches('/'),
        endpoint_path
    );

    if let Some(query) = query {
        target_url.push('?');
        target_url.push_str(query);
    }

    match ureq::request(request.method().as_str(), &target_url).call() {
        Ok(response) => {
            let response = build_response(response)?;
            request.respond(response)?;
        }
        Err(ureq::Error::Status(_, response)) => {
            let response = build_response(response)?;
            request.respond(response)?;
        }
        Err(error) => {
            eprintln!("Error contacting service '{}': {}", service.name, error);
            let response = Response::from_string("upstream error").with_status_code(502);
            request.respond(response)?;
        }
    }

    Ok(())
}

fn handle_internal_service_request(
    services: &[Service],
    logs: &SharedLogMap,
    request: Request,
    rest: &str,
) -> Result<()> {
    let mut segments = rest.split('/').filter(|segment| !segment.is_empty());
    let Some(service_name) = segments.next() else {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    };

    let Some(action) = segments.next() else {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    };

    if !services.iter().any(|service| service.name == service_name) {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    match action {
        "logs" => {
            let body = match logs.lock() {
                Ok(store) => match store.get(service_name) {
                    Some(lines) if !lines.is_empty() => {
                        lines.iter().cloned().collect::<Vec<_>>().join("\n")
                    }
                    Some(_) => "No hay logs disponibles aún.".to_string(),
                    None => {
                        let response = Response::from_string("not found").with_status_code(404);
                        request.respond(response)?;
                        return Ok(());
                    }
                },
                Err(_) => {
                    let response =
                        Response::from_string("log store unavailable").with_status_code(503);
                    request.respond(response)?;
                    return Ok(());
                }
            };

            let response = Response::from_string(body).with_status_code(200);
            request.respond(response)?;
        }
        "openapi" => {
            let path = crate::config::openapi_path(service_name);
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    let response = Response::from_string(contents).with_status_code(200);
                    request.respond(response)?;
                }
                Err(error) => {
                    eprintln!(
                        "No se pudo leer el OpenAPI del servicio '{}' en {}: {}",
                        service_name,
                        path.display(),
                        error
                    );
                    let response = Response::from_string("openapi not found").with_status_code(500);
                    request.respond(response)?;
                }
            }
        }
        _ => {
            let response = Response::from_string("not found").with_status_code(404);
            request.respond(response)?;
        }
    }

    Ok(())
}

fn resolve_service_route<'a>(
    services: &'a [Service],
    trimmed_path: &str,
) -> Option<(&'a Service, String)> {
    for service in services {
        if let Some(rest) = trimmed_path.strip_prefix(&service.prefix) {
            let endpoint_path = rest.trim_start_matches('/');
            if endpoint_path.is_empty() {
                continue;
            }
            return Some((service, endpoint_path.to_string()));
        }
    }
    None
}

fn build_response(upstream: ureq::Response) -> Result<Response<Cursor<Vec<u8>>>> {
    let status = upstream.status();
    let content_type = upstream
        .header("Content-Type")
        .map(|value| value.to_owned());
    let body = upstream
        .into_string()
        .map_err(|error| anyhow!("failed to read upstream response body: {error}"))?;

    let mut response = Response::from_string(body).with_status_code(status);

    if let Some(content_type) = content_type {
        if let Ok(header) = Header::from_bytes(b"Content-Type", content_type.as_bytes()) {
            response = response.with_header(header);
        }
    }

    Ok(response)
}

fn render_homepage(services: &[Service], health: &SharedHealthMap) -> Response<Cursor<Vec<u8>>> {
    let service_section = if services.is_empty() {
        "<p>No hay servicios cargados actualmente.</p>".to_string()
    } else {
        let mut items = String::new();
        let health_snapshot = health.lock().map(|map| map.clone()).unwrap_or_default();
        for service in services {
            let health_info = health_snapshot
                .get(&service.name)
                .copied()
                .unwrap_or_default();
            let (status_label, status_class) = match health_info.status {
                HealthStatus::Healthy => ("🟢 En línea", "status status--healthy"),
                HealthStatus::Unhealthy => ("🔴 Fuera de servicio", "status status--unhealthy"),
                HealthStatus::Unknown => ("⚪️ Sin datos", "status status--unknown"),
            };
            let last_checked = match health_info.last_checked {
                Some(instant) => {
                    let seconds = instant.elapsed().as_secs();
                    match seconds {
                        0 => "Última verificación hace menos de un segundo".to_string(),
                        1 => "Última verificación hace 1 segundo".to_string(),
                        _ => format!("Última verificación hace {} segundos", seconds),
                    }
                }
                None => "Última verificación pendiente".to_string(),
            };

            let item = format!(
                concat!(
                    "<li>",
                    "  <div class=\"service-header\">",
                    "    <strong>{name}</strong>",
                    "    <span class=\"service-actions\">",
                    "      <button type=\"button\" class=\"icon-button\" data-action=\"logs\" data-service=\"{name}\" title=\"Ver logs\" aria-label=\"Ver logs de {name}\">📜</button>",
                    "      <button type=\"button\" class=\"icon-button\" data-action=\"openapi\" data-service=\"{name}\" title=\"Ver OpenAPI\" aria-label=\"Ver OpenAPI de {name}\">📘</button>",
                    "    </span>",
                    "  </div>",
                    "  <span class=\"{status_class}\">{status_label}</span><br/>",
                    "  <span>Prefijo: <code>{prefix}</code></span><br/>",
                    "  <span>Base URL: <code>{base_url}</code></span><br/>",
                    "  <small>{last_checked}</small>",
                    "</li>"
                ),
                name = service.name,
                status_class = status_class,
                status_label = status_label,
                prefix = service.prefix,
                base_url = service.base_url,
                last_checked = last_checked
            );

            items.push_str(&item);
        }

        format!("<ul class=\"service-list\">{}</ul>", items)
    };

    let html = format!(
        concat!(
            "<!DOCTYPE html>\n",
            "<html lang=\"es\">\n",
            "<head>\n",
            "    <meta charset=\"utf-8\" />\n",
            "    <title>Servicios disponibles</title>\n",
            "    <link rel=\"stylesheet\" href=\"https://cdn.jsdelivr.net/npm/water.css@2/out/water.css\" />\n",
            "    <style>\n",
            "      .service-list {{ list-style: none; padding: 0; }}\n",
            "      .service-list li {{ margin-bottom: 1.5rem; }}\n",
            "      .service-header {{ display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; }}\n",
            "      .service-actions {{ display: inline-flex; align-items: center; gap: 0.25rem; }}\n",
            "      .icon-button {{ border: none; background: none; cursor: pointer; font-size: 1.25rem; line-height: 1; padding: 0.1rem; }}\n",
            "      .icon-button:focus {{ outline: 2px solid #4a90e2; outline-offset: 2px; }}\n",
            "      .status {{ font-weight: bold; display: inline-block; margin-bottom: 0.25rem; }}\n",
            "      .status--healthy {{ color: #0a7d24; }}\n",
            "      .status--unhealthy {{ color: #c62828; }}\n",
            "      .status--unknown {{ color: #616161; }}\n",
            "      .modal {{ position: fixed; inset: 0; background-color: rgba(0, 0, 0, 0.4); display: flex; align-items: center; justify-content: center; padding: 1rem; z-index: 1000; }}\n",
            "      .modal[hidden] {{ display: none; }}\n",
            "      .modal__dialog {{ background: #ffffff; color: #000000; width: min(90vw, 720px); max-height: 80vh; border-radius: 8px; box-shadow: 0 20px 45px rgba(0, 0, 0, 0.2); display: flex; flex-direction: column; overflow: hidden; }}\n",
            "      .modal__header {{ display: flex; align-items: center; justify-content: space-between; padding: 0.75rem 1rem; border-bottom: 1px solid #e0e0e0; }}\n",
            "      .modal__title {{ margin: 0; font-size: 1.1rem; }}\n",
            "      .modal__close {{ border: none; background: none; font-size: 1.5rem; line-height: 1; cursor: pointer; }}\n",
            "      .modal__body {{ padding: 1rem; overflow: auto; }}\n",
            "      .modal__body pre {{ margin: 0; font-size: 0.9rem; white-space: pre-wrap; word-break: break-word; }}\n",
            "    </style>\n",
            "</head>\n",
            "<body>\n",
            "    <main>\n",
            "      <h1>Servicios disponibles</h1>\n",
            "        <p>Estos son los servicios registrados actualmente en el runner.</p>\n",
            "        {service_section}\n",
            "    </main>\n",
            "    <div id=\"modal\" class=\"modal\" hidden>\n",
            "      <div class=\"modal__dialog\" role=\"dialog\" aria-modal=\"true\" aria-labelledby=\"modal-title\">\n",
            "        <div class=\"modal__header\">\n",
            "          <h2 id=\"modal-title\" class=\"modal__title\"></h2>\n",
            "          <button type=\"button\" class=\"modal__close\" aria-label=\"Cerrar\">✖</button>\n",
            "        </div>\n",
            "        <div class=\"modal__body\">\n",
            "          <pre id=\"modal-content\"></pre>\n",
            "        </div>\n",
            "      </div>\n",
            "    </div>\n",
            "    <script>\n",
            "      (function() {{\n",
            "        const modal = document.getElementById('modal');\n",
            "        const titleEl = document.getElementById('modal-title');\n",
            "        const contentEl = document.getElementById('modal-content');\n",
            "        const closeBtn = modal.querySelector('.modal__close');\n",
            "        const ACTION_LABELS = {{ logs: 'Logs', openapi: 'OpenAPI' }};\n",
            "\n",
            "        function closeModal() {{\n",
            "          modal.setAttribute('hidden', 'hidden');\n",
            "          contentEl.textContent = '';\n",
            "        }}\n",
            "\n",
            "        async function openModal(action, service) {{\n",
            "          const label = ACTION_LABELS[action] || 'Detalles';\n",
            "          titleEl.textContent = label + ' — ' + service;\n",
            "          contentEl.textContent = 'Cargando...';\n",
            "          modal.removeAttribute('hidden');\n",
            "\n",
            "          try {{\n",
            "            const response = await fetch('/__runner__/services/' + encodeURIComponent(service) + '/' + action);\n",
            "            if (!response.ok) {{\n",
            "              const text = await response.text();\n",
            "              throw new Error(text || ('Error ' + response.status));\n",
            "            }}\n",
            "            let text = await response.text();\n",
            "            if (action === 'openapi' && text) {{\n",
            "              try {{\n",
            "                const parsed = JSON.parse(text);\n",
            "                text = JSON.stringify(parsed, null, 2);\n",
            "              }} catch (_) {{\n",
            "                // dejar el texto tal cual\n",
            "              }}\n",
            "            }}\n",
            "            contentEl.textContent = text || 'Sin contenido disponible.';\n",
            "          }} catch (error) {{\n",
            "            contentEl.textContent = 'Error al cargar: ' + error.message;\n",
            "          }}\n",
            "        }}\n",
            "\n",
            "        document.querySelectorAll('.icon-button').forEach((button) => {{\n",
            "          button.addEventListener('click', () => {{\n",
            "            const action = button.getAttribute('data-action');\n",
            "            const service = button.getAttribute('data-service');\n",
            "            if (action && service) {{\n",
            "              openModal(action, service);\n",
            "            }}\n",
            "          }});\n",
            "        }});\n",
            "\n",
            "        closeBtn.addEventListener('click', closeModal);\n",
            "        modal.addEventListener('click', (event) => {{\n",
            "          if (event.target === modal) {{\n",
            "            closeModal();\n",
            "          }}\n",
            "        }});\n",
            "        document.addEventListener('keydown', (event) => {{\n",
            "          if (event.key === 'Escape' && !modal.hasAttribute('hidden')) {{\n",
            "            closeModal();\n",
            "          }}\n",
            "        }});\n",
            "      }})();\n",
            "    </script>\n",
            "</body>\n",
            "</html>\n"
        ),
        service_section = service_section
    );

    let mut response = Response::from_string(html);
    if let Ok(header) = Header::from_bytes(b"Content-Type", b"text/html; charset=utf-8") {
        response = response.with_header(header);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_service_route_matches_prefix() {
        let service = Service {
            name: "svc".into(),
            prefix: "svc".into(),
            base_url: "http://localhost:1234".into(),
            memory_limit_bytes: 64 * 1024 * 1024,
            allowed_get_endpoints: Default::default(),
        };
        let services = vec![service];

        let result = resolve_service_route(&services, "svc/ping");
        assert!(result.is_some());
        let (service, endpoint) = result.unwrap();
        assert_eq!(service.name, "svc");
        assert_eq!(endpoint, "ping");
    }

    #[test]
    fn resolve_service_route_ignores_empty_endpoint() {
        let service = Service {
            name: "svc".into(),
            prefix: "svc".into(),
            base_url: "http://localhost:1234".into(),
            memory_limit_bytes: 64 * 1024 * 1024,
            allowed_get_endpoints: Default::default(),
        };
        let services = vec![service];

        assert!(resolve_service_route(&services, "svc/").is_none());
    }
}
