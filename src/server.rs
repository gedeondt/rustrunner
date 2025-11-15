use anyhow::{anyhow, Result};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::time::Instant;
use std::time::SystemTime;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::config::Service;
#[cfg(test)]
use crate::config::ServiceKind;
use crate::health::{HealthStatus, SharedHealthMap};
use crate::logs::SharedLogMap;
use crate::memory::{ServiceMemorySnapshot, SharedMemoryMap};
use crate::queue::{with_queue_registry, QueueSnapshot, SharedQueueRegistry};
use crate::scheduler::{self, ScheduleState, SharedScheduleMap, ToggleError, TriggerError};
use crate::stats::{record_http_status, SharedStats};
use crate::templates;
use serde_json::json;

const ENTRY_PORT: u16 = 14000;

pub fn run_server(
    services: &[Service],
    health: &SharedHealthMap,
    logs: &SharedLogMap,
    schedules: &SharedScheduleMap,
    stats: &SharedStats,
    queues: &SharedQueueRegistry,
    memory: &SharedMemoryMap,
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
        if let Err(error) = handle_request(
            services, health, logs, schedules, stats, queues, memory, request,
        ) {
            eprintln!("Failed to handle request: {:#}", error);
        }
    }

    Ok(())
}

fn handle_request(
    services: &[Service],
    health: &SharedHealthMap,
    logs: &SharedLogMap,
    schedules: &SharedScheduleMap,
    stats: &SharedStats,
    queues: &SharedQueueRegistry,
    memory: &SharedMemoryMap,
    request: Request,
) -> Result<()> {
    let full_path = request.url().to_owned();
    let (path, query) = match full_path.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (full_path.as_str(), None),
    };

    let trimmed_path = path.trim_start_matches('/');

    if request.method() == &Method::Post {
        if let Some(queue_name) = trimmed_path.strip_prefix("__runner__/queues/") {
            return handle_queue_publish(queues, request, queue_name);
        }

        if let Some(rest) = trimmed_path.strip_prefix("__runner__/services/") {
            return handle_internal_service_control(services, schedules, request, rest);
        }

        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    if request.method() != &Method::Get {
        let response = Response::from_string("method not allowed").with_status_code(405);
        request.respond(response)?;
        return Ok(());
    }

    if trimmed_path == "health" {
        let response = Response::from_string("ok").with_status_code(200);
        request.respond(response)?;
        return Ok(());
    }

    if trimmed_path.is_empty() {
        let response = render_homepage(services, health, queues, schedules, memory);
        request.respond(response)?;
        return Ok(());
    }

    if trimmed_path == "__runner__/stats" {
        return handle_stats_request(stats, request);
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
            let status = response.status();
            record_http_status(stats, &service.name, &endpoint_path, status as u16);
            let response = build_response(response)?;
            request.respond(response)?;
        }
        Err(ureq::Error::Status(_, response)) => {
            let status = response.status();
            record_http_status(stats, &service.name, &endpoint_path, status as u16);
            let response = build_response(response)?;
            request.respond(response)?;
        }
        Err(error) => {
            eprintln!("Error contacting service '{}': {}", service.name, error);
            record_http_status(stats, &service.name, &endpoint_path, 502);
            let response = Response::from_string("upstream error").with_status_code(502);
            request.respond(response)?;
        }
    }

    Ok(())
}

fn handle_stats_request(stats: &SharedStats, request: Request) -> Result<()> {
    let snapshot = match stats.lock() {
        Ok(store) => store.snapshot(SystemTime::now()),
        Err(_) => {
            let response = Response::from_string("stats unavailable").with_status_code(503);
            request.respond(response)?;
            return Ok(());
        }
    };

    let body = serde_json::to_string(&snapshot)
        .map_err(|error| anyhow!("failed to serialize stats snapshot: {error}"))?;

    let mut response = Response::from_string(body).with_status_code(200);
    if let Ok(header) = Header::from_bytes(b"Content-Type", b"application/json; charset=utf-8") {
        response = response.with_header(header);
    }

    request.respond(response)?;
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

    let remaining: Vec<_> = segments.collect();

    match action {
        "logs" => {
            if !remaining.is_empty() {
                let response = Response::from_string("not found").with_status_code(404);
                request.respond(response)?;
                return Ok(());
            }
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
            if !remaining.is_empty() {
                let response = Response::from_string("not found").with_status_code(404);
                request.respond(response)?;
                return Ok(());
            }
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
        "schedules" => {
            let response = Response::from_string("method not allowed").with_status_code(405);
            request.respond(response)?;
        }
        _ => {
            let response = Response::from_string("not found").with_status_code(404);
            request.respond(response)?;
        }
    }

    Ok(())
}

fn handle_internal_service_control(
    services: &[Service],
    schedules: &SharedScheduleMap,
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

    if action != "schedules" {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    if !services.iter().any(|service| service.name == service_name) {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    let remaining: Vec<_> = segments.collect();
    handle_schedule_request(services, service_name, schedules, request, &remaining)
}

fn handle_queue_publish(
    queues: &SharedQueueRegistry,
    mut request: Request,
    raw_queue_name: &str,
) -> Result<()> {
    let queue_name = raw_queue_name.trim();

    if queue_name.is_empty() {
        let response = Response::from_string("invalid queue name").with_status_code(400);
        request.respond(response)?;
        return Ok(());
    }

    let content_type = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Content-Type"))
        .map(|header| header.value.to_string());

    let mut payload = Vec::new();
    if let Err(error) = request.as_reader().read_to_end(&mut payload) {
        eprintln!("Failed to read queue payload: {}", error);
        let response = Response::from_string("invalid payload").with_status_code(500);
        request.respond(response)?;
        return Ok(());
    }

    let (subscribers, message_count) =
        match with_queue_registry(queues, |registry| registry.prepare_delivery(queue_name)) {
            Ok(result) => result,
            Err(error) => {
                eprintln!("Failed to access queue registry: {error}");
                let response = Response::from_string("queue unavailable").with_status_code(500);
                request.respond(response)?;
                return Ok(());
            }
        };

    for subscriber in &subscribers {
        let mut call = ureq::post(&subscriber.target_url).set("X-Rustrunner-Queue", queue_name);

        if let Some(ref ct) = content_type {
            call = call.set("Content-Type", ct);
        } else {
            call = call.set("Content-Type", "application/json");
        }

        if let Err(error) = call.send_bytes(&payload) {
            eprintln!(
                "Failed to deliver queue '{}' event to service '{}' at {}: {}",
                queue_name, subscriber.service_name, subscriber.target_url, error
            );
        }
    }

    let response_body = json!({
        "queue": queue_name,
        "subscribers": subscribers.len(),
        "message_count": message_count,
    })
    .to_string();

    let mut response = Response::from_string(response_body).with_status_code(202);
    if let Ok(header) = Header::from_bytes(b"Content-Type", b"application/json; charset=utf-8") {
        response = response.with_header(header);
    }

    request.respond(response)?;
    Ok(())
}

fn handle_schedule_request(
    services: &[Service],
    service_name: &str,
    schedules: &SharedScheduleMap,
    request: Request,
    remaining: &[&str],
) -> Result<()> {
    if remaining.len() != 2 {
        let response = Response::from_string("not found").with_status_code(404);
        request.respond(response)?;
        return Ok(());
    }

    let index: usize = match remaining[0].parse() {
        Ok(value) => value,
        Err(_) => {
            let response = Response::from_string("invalid schedule index").with_status_code(400);
            request.respond(response)?;
            return Ok(());
        }
    };

    match remaining[1] {
        "toggle" => match scheduler::toggle_schedule(schedules, service_name, index) {
            Ok(paused) => {
                let payload = json!({ "paused": paused });
                let mut response = Response::from_string(payload.to_string()).with_status_code(200);
                if let Ok(header) = Header::from_bytes(b"Content-Type", b"application/json") {
                    response = response.with_header(header);
                }
                request.respond(response)?;
            }
            Err(ToggleError::ServiceNotFound | ToggleError::ScheduleNotFound) => {
                let response = Response::from_string("not found").with_status_code(404);
                request.respond(response)?;
            }
            Err(ToggleError::LockPoisoned) => {
                let response =
                    Response::from_string("schedule controller unavailable").with_status_code(503);
                request.respond(response)?;
            }
        },
        "run" => {
            let Some(service) = services.iter().find(|svc| svc.name == service_name) else {
                let response = Response::from_string("not found").with_status_code(404);
                request.respond(response)?;
                return Ok(());
            };

            let Some(schedule_config) = service.schedules.get(index) else {
                let response = Response::from_string("not found").with_status_code(404);
                request.respond(response)?;
                return Ok(());
            };

            match scheduler::trigger_schedule_now(
                schedules,
                service_name,
                index,
                &service.base_url,
                &schedule_config.endpoint,
            ) {
                Ok(outcome) => {
                    let status_text = match (&outcome.last_error, outcome.last_status) {
                        (Some(error), _) => format!("Último error: {error}"),
                        (None, Some(status)) => format!("Último HTTP: {status}"),
                        (None, None) => "Aún no se ha ejecutado.".to_string(),
                    };
                    let time_text = match outcome.last_run {
                        Some(instant) => describe_elapsed("Última ejecución", instant),
                        None => "Pendiente de la primera ejecución".to_string(),
                    };
                    let payload = json!({
                        "status_text": status_text,
                        "time_text": time_text,
                    });
                    let mut response =
                        Response::from_string(payload.to_string()).with_status_code(200);
                    if let Ok(header) = Header::from_bytes(b"Content-Type", b"application/json") {
                        response = response.with_header(header);
                    }
                    request.respond(response)?;
                }
                Err(TriggerError::ServiceNotFound | TriggerError::ScheduleNotFound) => {
                    let response = Response::from_string("not found").with_status_code(404);
                    request.respond(response)?;
                }
                Err(TriggerError::LockPoisoned) => {
                    let response = Response::from_string("schedule controller unavailable")
                        .with_status_code(503);
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

fn render_domain_sections(
    services: &[Service],
    health: &SharedHealthMap,
    schedules: &SharedScheduleMap,
    memory: &SharedMemoryMap,
) -> String {
    let health_snapshot = health.lock().map(|map| map.clone()).unwrap_or_default();
    let schedule_snapshot = schedules.lock().map(|map| map.clone()).unwrap_or_default();
    let memory_snapshot = memory.lock().map(|map| map.clone()).unwrap_or_default();
    let mut groups: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();

    for service in services {
        let health_info = health_snapshot
            .get(&service.name)
            .copied()
            .unwrap_or_default();
        let status_badge = render_status_badge(health_info.status);
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
        let schedule_section =
            build_schedule_section(&service.name, schedule_snapshot.get(&service.name));
        let memory_info = memory_snapshot
            .get(&service.name)
            .copied()
            .unwrap_or_default();
        let memory_section = render_memory_section(&memory_info);

        let card = render_service_card(
            service,
            status_badge.as_str(),
            last_checked.as_str(),
            memory_section.as_str(),
            schedule_section.as_str(),
        );
        groups
            .entry(service.domain.clone())
            .or_default()
            .entry(service.kind.label().to_string())
            .or_default()
            .push(card);
    }

    let mut output = String::new();

    for (domain, categories) in groups {
        let mut category_entries: Vec<(String, Vec<String>)> = categories.into_iter().collect();
        let count: usize = category_entries.iter().map(|(_, cards)| cards.len()).sum();
        let domain_title = humanize_domain(&domain);
        let count_label = if count == 1 {
            format!("{count} servicio en este dominio")
        } else {
            format!("{count} servicios en este dominio")
        };
        let rank = |label: &str| match label {
            "BFF" => 0,
            "Business" => 1,
            "Adapter" => 2,
            _ => 3,
        };

        category_entries.sort_by(|(a, _), (b, _)| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
        let mut category_sections = String::new();

        for (category, cards) in category_entries {
            let summary_label = format!("{category} ({})", cards.len());
            let services_html = cards.join("");
            category_sections.push_str(&format!(
                concat!(
                    "<details class=\"group rounded-xl border border-slate-800 bg-slate-900/40\" open>",
                    "  <summary class=\"flex cursor-pointer items-center justify-between gap-2 px-4 py-3 text-sm font-semibold text-slate-200 [&::-webkit-details-marker]:hidden\">",
                    "    <span>{summary}</span>",
                    "    <span class=\"text-slate-500 transition-transform group-open:-rotate-180\">⌄</span>",
                    "  </summary>",
                    "  <div class=\"px-4 pb-4\">",
                    "    <ul class=\"mt-4 grid gap-4 lg:grid-cols-2\">{services}</ul>",
                    "  </div>",
                    "</details>"
                ),
                summary = escape_html(&summary_label),
                services = services_html
            ));
        }

        output.push_str(&format!(
            concat!(
                "<section class=\"rounded-2xl border border-slate-800 bg-slate-900/60 shadow-glow shadow-slate-950/30\">",
                "  <details class=\"group\" open>",
                "    <summary class=\"flex cursor-pointer items-center justify-between gap-4 px-6 py-5 text-left [&::-webkit-details-marker]:hidden\">",
                "      <div>",
                "        <h2 class=\"text-2xl font-semibold text-white\">Dominio {domain}</h2>",
                "        <p class=\"text-sm text-slate-400\">{count_label}</p>",
                "      </div>",
                "      <span class=\"text-slate-500 transition-transform group-open:-rotate-180\">⌄</span>",
                "    </summary>",
                "    <div class=\"border-t border-slate-800/60 px-6 pb-6 pt-4\">",
                "      <div class=\"space-y-4\">{categories}</div>",
                "    </div>",
                "  </details>",
                "</section>"
            ),
            domain = escape_html(&domain_title),
            count_label = escape_html(&count_label),
            categories = category_sections
        ));
    }

    output
}

fn render_service_card(
    service: &Service,
    status_badge: &str,
    last_checked: &str,
    memory_section: &str,
    schedule_section: &str,
) -> String {
    let kind_label = service.kind.label();

    format!(
        concat!(
            "<li class=\"rounded-2xl border border-slate-800 bg-slate-900/50 p-5 shadow shadow-slate-950/30\">",
            "  <div class=\"flex flex-col gap-4\">",
            "    <div class=\"flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between\">",
            "      <div class=\"space-y-3\">",
            "        <div class=\"flex flex-wrap items-center gap-3\">",
            "          <h3 class=\"text-xl font-semibold text-white\">{name}</h3>",
            "          <span class=\"inline-flex items-center gap-2 rounded-full border border-slate-700 bg-slate-800/70 px-3 py-1 text-xs font-semibold uppercase tracking-wide text-slate-200\">{kind}</span>",
            "        </div>",
            "        <div class=\"flex flex-col gap-1 text-sm text-slate-400\">",
            "          <span>Prefijo: <code class=\"text-slate-200\">{prefix}</code></span>",
            "          <span>Base URL: <code class=\"text-slate-200\">{base_url}</code></span>",
            "        </div>",
            "      </div>",
            "      <div class=\"flex flex-col items-start gap-3 sm:items-end\">",
            "        {status_badge}",
            "        <div class=\"flex gap-2\">",
            "          <button type=\"button\" class=\"icon-button inline-flex items-center justify-center gap-2 rounded-lg border border-slate-700 bg-slate-900/70 px-3 py-2 text-sm font-medium text-slate-200 transition hover:bg-slate-900 focus:outline-none focus:ring focus:ring-slate-500/40\" data-action=\"logs\" data-service=\"{name_attr}\" title=\"Ver logs\" aria-label=\"Ver logs de {name_attr}\">📜</button>",
            "          <button type=\"button\" class=\"icon-button inline-flex items-center justify-center gap-2 rounded-lg border border-slate-700 bg-slate-900/70 px-3 py-2 text-sm font-medium text-slate-200 transition hover:bg-slate-900 focus:outline-none focus:ring focus:ring-slate-500/40\" data-action=\"openapi\" data-service=\"{name_attr}\" title=\"Ver OpenAPI\" aria-label=\"Ver OpenAPI de {name_attr}\">📘</button>",
            "        </div>",
            "      </div>",
            "    </div>",
            "    <p class=\"text-xs text-slate-500\">{last_checked}</p>",
            "    {memory_section}",
            "    {schedule_section}",
            "  </div>",
            "</li>"
        ),
        name = escape_html(&service.name),
        name_attr = escape_html(&service.name),
        kind = escape_html(kind_label),
        prefix = escape_html(&service.prefix),
        base_url = escape_html(&service.base_url),
        status_badge = status_badge,
        last_checked = escape_html(last_checked),
        memory_section = memory_section,
        schedule_section = schedule_section
    )
}

fn render_status_badge(status: HealthStatus) -> String {
    let (label, classes) = match status {
        HealthStatus::Healthy => (
            "🟢 En línea",
            "border-emerald-500/40 bg-emerald-500/10 text-emerald-200",
        ),
        HealthStatus::Unhealthy => (
            "🔴 Fuera de servicio",
            "border-rose-500/40 bg-rose-500/10 text-rose-200",
        ),
        HealthStatus::Unknown => (
            "⚪️ Sin datos",
            "border-slate-700 bg-slate-800/70 text-slate-300",
        ),
    };

    format!(
        "<span class=\"inline-flex items-center gap-2 rounded-full border px-3 py-1 text-xs font-semibold uppercase tracking-wide {classes}\">{label}</span>",
        classes = classes,
        label = label,
    )
}

fn humanize_domain(domain: &str) -> String {
    if domain.is_empty() {
        return String::new();
    }

    let mut words = Vec::new();
    let mut current = String::new();

    for ch in domain.chars() {
        if ch == '_' || ch == '-' {
            if !current.is_empty() {
                words.push(current.clone());
                current.clear();
            }
            continue;
        }

        if ch.is_uppercase() && !current.is_empty() {
            words.push(current.clone());
            current.clear();
        }

        current.push(ch);
    }

    if !current.is_empty() {
        words.push(current);
    }

    if words.is_empty() {
        return String::new();
    }

    words
        .into_iter()
        .map(|segment| {
            let mut chars = segment.chars();
            let mut label = String::new();
            if let Some(first) = chars.next() {
                for up in first.to_uppercase() {
                    label.push(up);
                }
                label.push_str(&chars.as_str().to_lowercase());
            }
            label
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_queue_section(queues: &SharedQueueRegistry) -> String {
    let snapshot = match with_queue_registry(queues, |registry| registry.snapshot()) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            return "<p class=\"text-sm text-rose-300\">No se pudo obtener el estado de las colas en este momento.</p>".to_string();
        }
    };

    if snapshot.is_empty() {
        return "<p class=\"text-sm text-slate-400\">Aún no se han instanciado colas.</p>"
            .to_string();
    }

    render_queue_table(&snapshot)
}

fn render_queue_table(snapshot: &[QueueSnapshot]) -> String {
    let mut rows = String::new();

    for queue in snapshot {
        rows.push_str(&format!(
            concat!(
                "<tr class=\"border-b border-slate-800/60 last:border-b-0\">",
                "  <td class=\"whitespace-nowrap px-4 py-3 font-medium text-slate-200\"><code>{}</code></td>",
                "  <td class=\"px-4 py-3 text-right text-slate-300\">{}</td>",
                "  <td class=\"px-4 py-3 text-right text-slate-300\">{}</td>",
                "</tr>"
            ),
            escape_html(&queue.name),
            queue.subscriber_count,
            queue.message_count
        ));
    }

    format!(
        concat!(
            "<div class=\"overflow-hidden rounded-xl border border-slate-800/80\">",
            "  <table class=\"min-w-full divide-y divide-slate-800/80 text-sm\">",
            "    <thead class=\"bg-slate-900/80 text-slate-300\">",
            "      <tr>",
            "        <th class=\"px-4 py-3 text-left font-semibold uppercase tracking-wider\">Cola</th>",
            "        <th class=\"px-4 py-3 text-right font-semibold uppercase tracking-wider\">Suscriptores</th>",
            "        <th class=\"px-4 py-3 text-right font-semibold uppercase tracking-wider\">Mensajes procesados</th>",
            "      </tr>",
            "    </thead>",
            "    <tbody class=\"bg-slate-950/30\">{}</tbody>",
            "  </table>",
            "</div>"
        ),
        rows
    )
}

fn build_schedule_section(service_name: &str, entries: Option<&Vec<ScheduleState>>) -> String {
    let Some(entries) = entries else {
        return concat!(
            "<div class=\"schedule-section rounded-xl border border-slate-800/80 bg-slate-950/40 p-4\">",
            "  <h4 class=\"text-sm font-semibold uppercase tracking-wide text-slate-300\">Webhooks programados</h4>",
            "  <p class=\"schedule-empty mt-2 text-sm text-slate-400\">No hay webhooks programados.</p>",
            "</div>"
        )
        .to_string();
    };

    if entries.is_empty() {
        return concat!(
            "<div class=\"schedule-section rounded-xl border border-slate-800/80 bg-slate-950/40 p-4\">",
            "  <h4 class=\"text-sm font-semibold uppercase tracking-wide text-slate-300\">Webhooks programados</h4>",
            "  <p class=\"schedule-empty mt-2 text-sm text-slate-400\">No hay webhooks programados.</p>",
            "</div>"
        )
        .to_string();
    }

    let mut items = String::new();

    for (index, state) in entries.iter().enumerate() {
        let endpoint_display = format!("/{}", state.endpoint);
        let state_label = if state.paused {
            "⏸️ Pausado"
        } else {
            "▶️ En ejecución"
        };
        let button_label = if state.paused { "Reanudar" } else { "Pausar" };
        let paused_attr = if state.paused { "true" } else { "false" };

        let status_text = if let Some(error) = &state.last_error {
            format!("Último error: {error}")
        } else if let Some(status) = state.last_status {
            format!("Último HTTP: {status}")
        } else {
            "Aún no se ha ejecutado.".to_string()
        };

        let time_text = match state.last_run {
            Some(instant) => describe_elapsed("Última ejecución", instant),
            None => "Pendiente de la primera ejecución".to_string(),
        };

        items.push_str(&format!(
            concat!(
                "<li class=\"schedule-item rounded-lg border border-slate-800/80 bg-slate-900/50 p-4\" data-service=\"{service}\" data-index=\"{index}\">",
                "  <div class=\"schedule-item__header flex flex-wrap gap-3 sm:items-center sm:justify-between\">",
                "    <div class=\"schedule-item__info flex min-w-[200px] flex-col gap-1\">",
                "      <span class=\"font-medium text-slate-200\"><code>{endpoint}</code></span>",
                "      <span class=\"schedule-item__meta text-sm text-slate-400\">Cada {interval}s · <span class=\"schedule-item__state font-semibold text-slate-200\">{state_label}</span></span>",
                "    </div>",
                "    <div class=\"schedule-item__actions flex flex-wrap gap-2\">",
                "      <button type=\"button\" class=\"schedule-run inline-flex items-center justify-center rounded-lg border border-emerald-500/40 bg-emerald-500/10 px-3 py-1.5 text-xs font-semibold uppercase tracking-wide text-emerald-200 transition hover:bg-emerald-500/20 focus:outline-none focus:ring focus:ring-emerald-500/40\" data-service=\"{service}\" data-index=\"{index}\">Lanzar ahora</button>",
                "      <button type=\"button\" class=\"schedule-toggle inline-flex items-center justify-center rounded-lg border border-slate-700 bg-slate-900/70 px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-slate-200 transition hover:bg-slate-900 focus:outline-none focus:ring focus:ring-slate-500/40\" data-service=\"{service}\" data-index=\"{index}\" data-paused=\"{paused}\">{button_label}</button>",
                "    </div>",
                "  </div>",
                "  <div class=\"schedule-item__details mt-3 flex flex-col gap-1 text-xs text-slate-400\">",
                "    <span class=\"schedule-item__result\">{status_text}</span>",
                "    <span class=\"schedule-item__time\">{time_text}</span>",
                "  </div>",
                "</li>"
            ),
            service = escape_html(service_name),
            index = index,
            endpoint = escape_html(&endpoint_display),
            interval = state.interval_secs,
            state_label = state_label,
            paused = paused_attr,
            button_label = button_label,
            status_text = escape_html(&status_text),
            time_text = escape_html(&time_text)
        ));
    }

    format!(
        concat!(
            "<div class=\"schedule-section rounded-xl border border-slate-800/80 bg-slate-950/40 p-4\">",
            "  <h4 class=\"text-sm font-semibold uppercase tracking-wide text-slate-300\">Webhooks programados</h4>",
            "  <ul class=\"schedule-list mt-3 flex flex-col gap-3\">{items}</ul>",
            "</div>"
        ),
        items = items
    )
}

fn render_memory_section(snapshot: &ServiceMemorySnapshot) -> String {
    let description = match (snapshot.usage_bytes, snapshot.limit_bytes) {
        (Some(usage), Some(limit)) if limit > 0 => {
            let percent = ((usage as f64) / (limit as f64)).min(1.0) * 100.0;
            format!(
                "{} / {} ({percent:.0}%)",
                format_bytes(usage),
                format_bytes(limit)
            )
        }
        (Some(usage), _) => format!("{} en uso", format_bytes(usage)),
        (None, Some(limit)) => format!("Límite configurado: {}", format_bytes(limit)),
        (None, None) => "Sin datos de consumo".to_string(),
    };

    let progress = match (snapshot.usage_bytes, snapshot.limit_bytes) {
        (Some(usage), Some(limit)) if limit > 0 => {
            let percent = ((usage as f64) / (limit as f64)).min(1.0) * 100.0;
            format!(
                concat!(
                    "<div class=\"mt-2 h-2 w-full rounded-full bg-slate-800/80\">",
                    "  <div class=\"h-full rounded-full bg-emerald-400\" style=\"width: {percent:.0}%\"></div>",
                    "</div>",
                    "<p class=\"mt-1 text-right text-xs text-slate-500\">{percent:.0}% del límite</p>"
                ),
                percent = percent
            )
        }
        _ => String::new(),
    };

    let updated = snapshot.last_updated.map(|instant| {
        let text = describe_elapsed("Actualizado", instant);
        format!(
            "<p class=\"mt-1 text-xs text-slate-500\">{}</p>",
            escape_html(&text)
        )
    });

    format!(
        concat!(
            "<div class=\"rounded-2xl border border-slate-800/80 bg-slate-950/40 p-4\">",
            "  <p class=\"text-xs font-semibold uppercase tracking-wide text-slate-400\">Memoria</p>",
            "  <p class=\"mt-1 text-sm text-slate-200\">{description}</p>",
            "  {progress}",
            "  {updated}",
            "</div>"
        ),
        description = escape_html(&description),
        progress = progress,
        updated = updated.unwrap_or_default()
    )
}

fn describe_elapsed(prefix: &str, instant: Instant) -> String {
    let seconds = instant.elapsed().as_secs();
    match seconds {
        0 => format!("{prefix} hace menos de un segundo"),
        1 => format!("{prefix} hace 1 segundo"),
        _ => format!("{prefix} hace {seconds} segundos"),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let value = bytes as f64;

    if bytes >= GB as u64 {
        format!("{:.1} GB", value / GB)
    } else if bytes >= MB as u64 {
        format!("{:.1} MB", value / MB)
    } else if bytes >= KB as u64 {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
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

fn render_homepage(
    services: &[Service],
    health: &SharedHealthMap,
    queues: &SharedQueueRegistry,
    schedules: &SharedScheduleMap,
    memory: &SharedMemoryMap,
) -> Response<Cursor<Vec<u8>>> {
    let service_section = if services.is_empty() {
        concat!(
            "<section class=\"rounded-2xl border border-slate-800 bg-slate-900/60 p-6 shadow-glow shadow-slate-950/30\">",
            "  <p class=\"text-sm text-slate-400\">No hay servicios cargados actualmente.</p>",
            "</section>"
        )
        .to_string()
    } else {
        render_domain_sections(services, health, schedules, memory)
    };

    let queue_section = render_queue_section(queues);

    let html = templates::render(
        templates::DASHBOARD,
        &[
            ("service_section", service_section.as_str()),
            ("queue_section", queue_section.as_str()),
        ],
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
            domain: "demo".into(),
            kind: ServiceKind::Business,
            prefix: "svc".into(),
            base_url: "http://localhost:1234".into(),
            allowed_get_endpoints: Default::default(),
            queue_listeners: Vec::new(),
            schedules: Vec::new(),
            memory_limit_mb: None,
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
            domain: "demo".into(),
            kind: ServiceKind::Adapter,
            prefix: "svc".into(),
            base_url: "http://localhost:1234".into(),
            allowed_get_endpoints: Default::default(),
            queue_listeners: Vec::new(),
            schedules: Vec::new(),
            memory_limit_mb: None,
        };
        let services = vec![service];

        assert!(resolve_service_route(&services, "svc/").is_none());
    }
}
