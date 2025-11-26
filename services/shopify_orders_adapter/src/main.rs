use std::convert::Infallible;
use std::env;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use env_logger::{Builder, Env, Target};
use hyper::body;
use hyper::client::HttpConnector;
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, Method, Request, Response, Server, StatusCode};
use log::{error, info, warn};
use serde_json::json;

type HttpResponse = Response<Body>;
type HandlerResult = Result<HttpResponse, Infallible>;

#[derive(Clone)]
struct OrderLineItem {
    title: &'static str,
    sku: &'static str,
    quantity: u32,
    price: f64,
}

#[derive(Clone)]
struct Order {
    id: &'static str,
    name: &'static str,
    created_at: &'static str,
    currency: &'static str,
    total_price: f64,
    financial_status: &'static str,
    fulfillment_status: &'static str,
    customer_name: &'static str,
    customer_email: &'static str,
    tags: &'static [&'static str],
    items: &'static [OrderLineItem],
    new_order: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct EndpointResponse {
    status: u16,
    body: String,
    content_type: &'static str,
}

const SERVICE_NAME: &str = "shopify-orders-adapter";
const DEFAULT_PORT: u16 = 15004;
const DEFAULT_RUNNER_BASE_URL: &str = "http://127.0.0.1:14000";
const SHOPIFY_NEW_ORDER_QUEUE: &str = "shopify.pedidos.nuevos";
const SHOPIFY_NEW_ORDER_EVENT: &str = "ShopifyOrderCreated";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    Builder::from_env(Env::default().default_filter_or("info"))
        .format(|buf, record| writeln!(buf, "[{}] {}", record.level(), record.args()))
        .target(Target::Stdout)
        .init();

    let port = resolve_port();
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
    info!(
        "Service '{SERVICE_NAME}' listening on http://{}:{}",
        "0.0.0.0", port
    );

    if let Err(error) = run_http_server(bind_addr).await {
        error!("HTTP server exited with error: {error}");
    }
}

fn resolve_port() -> u16 {
    env::var("WR_RUNNER_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_PORT)
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

    let Some(result) = dispatch_endpoint(&segments).await else {
        return Ok(text_response(StatusCode::NOT_FOUND, "not found"));
    };

    Ok(endpoint_response(result))
}

async fn dispatch_endpoint(segments: &[&str]) -> Option<EndpointResponse> {
    match segments {
        ["health"] => Some(EndpointResponse {
            status: 200,
            body: "ok".into(),
            content_type: "text/plain; charset=utf-8",
        }),
        ["shopify", "orders"] => Some(json_response(list_orders_payload())),
        ["shopify", "orders", order_id] => find_order(order_id).map(|order| json_response(order_detail(&order))),
        ["webhooks", "orders", "pull"] => Some(trigger_orders_webhook().await),
        _ => None,
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

fn json_response(payload: serde_json::Value) -> EndpointResponse {
    EndpointResponse {
        status: 200,
        body: payload.to_string(),
        content_type: "application/json",
    }
}

fn list_orders_payload() -> serde_json::Value {
    let orders: Vec<_> = seed_orders().into_iter().map(order_summary).collect();
    json!({ "orders": orders })
}

fn order_summary(order: Order) -> serde_json::Value {
    json!({
        "id": order.id,
        "name": order.name,
        "createdAt": order.created_at,
        "totalPrice": order.total_price,
        "currency": order.currency,
        "financialStatus": order.financial_status,
        "fulfillmentStatus": order.fulfillment_status,
        "customer": {
            "name": order.customer_name,
            "email": order.customer_email
        },
        "tags": order.tags
    })
}

fn order_detail(order: &Order) -> serde_json::Value {
    let items: Vec<_> = order
        .items
        .iter()
        .map(|item| {
            json!({
                "title": item.title,
                "sku": item.sku,
                "quantity": item.quantity,
                "price": item.price
            })
        })
        .collect();

    json!({
        "id": order.id,
        "name": order.name,
        "createdAt": order.created_at,
        "totalPrice": order.total_price,
        "currency": order.currency,
        "financialStatus": order.financial_status,
        "fulfillmentStatus": order.fulfillment_status,
        "customer": {
            "name": order.customer_name,
            "email": order.customer_email
        },
        "tags": order.tags,
        "lineItems": items
    })
}

fn seed_orders() -> Vec<Order> {
    vec![
        Order {
            id: "gid://shopify/Order/1001",
            name: "#1001",
            created_at: "2024-11-20T08:15:00Z",
            currency: "EUR",
            total_price: 189.5,
            financial_status: "paid",
            fulfillment_status: "fulfilled",
            customer_name: "Lucía Hernández",
            customer_email: "lucia.hernandez@example.com",
            tags: &["online", "priority"],
            items: &[
                OrderLineItem {
                    title: "Sudadera Vintage",
                    sku: "SUD-001-BLK",
                    quantity: 1,
                    price: 89.5,
                },
                OrderLineItem {
                    title: "Zapatillas Runner",
                    sku: "RUN-842-BLU",
                    quantity: 1,
                    price: 100.0,
                },
            ],
            new_order: true,
        },
        Order {
            id: "gid://shopify/Order/1002",
            name: "#1002",
            created_at: "2024-11-20T09:05:00Z",
            currency: "EUR",
            total_price: 78.0,
            financial_status: "pending",
            fulfillment_status: "unfulfilled",
            customer_name: "Diego Martín",
            customer_email: "diego.martin@example.com",
            tags: &["mobile", "express-shipping"],
            items: &[OrderLineItem {
                title: "Camisa Oxford",
                sku: "SHIRT-010-WHT",
                quantity: 2,
                price: 39.0,
            }],
            new_order: true,
        },
        Order {
            id: "gid://shopify/Order/0998",
            name: "#0998",
            created_at: "2024-11-18T19:30:00Z",
            currency: "EUR",
            total_price: 240.75,
            financial_status: "paid",
            fulfillment_status: "partial",
            customer_name: "Sonia Pérez",
            customer_email: "sonia.perez@example.com",
            tags: &["loyal-customer"],
            items: &[
                OrderLineItem {
                    title: "Abrigo Parka",
                    sku: "PRK-221-GRN",
                    quantity: 1,
                    price: 150.75,
                },
                OrderLineItem {
                    title: "Bufanda Lana",
                    sku: "SCF-010-GRY",
                    quantity: 1,
                    price: 90.0,
                },
            ],
            new_order: false,
        },
    ]
}

fn find_order(order_id: &str) -> Option<Order> {
    seed_orders()
        .into_iter()
        .find(|order| order.id == order_id || order.name.trim_start_matches('#') == order_id)
}

fn build_runner_publish_url() -> String {
    let runner_base_url = env::var("RUNNER_BASE_URL").unwrap_or_else(|_| DEFAULT_RUNNER_BASE_URL.to_string());
    format!(
        "{}/__runner__/queues/{}",
        runner_base_url.trim_end_matches('/'),
        SHOPIFY_NEW_ORDER_QUEUE
    )
}

async fn trigger_orders_webhook() -> EndpointResponse {
    let publish_url = build_runner_publish_url();
    let client = Client::new();
    let new_orders: Vec<Order> = seed_orders().into_iter().filter(|order| order.new_order).collect();

    if new_orders.is_empty() {
        return EndpointResponse {
            status: 204,
            body: String::new(),
            content_type: "text/plain; charset=utf-8",
        };
    }

    let mut published = Vec::new();
    let mut errors = Vec::new();

    for order in new_orders {
        match publish_order_event(&client, &publish_url, &order).await {
            Ok(status) => {
                published.push(json!({
                    "orderId": order.id,
                    "event": SHOPIFY_NEW_ORDER_EVENT,
                    "status": status
                }));
            }
            Err(error) => errors.push(json!({
                "orderId": order.id,
                "error": error
            })),
        }
    }

    if published.is_empty() {
        return EndpointResponse {
            status: 500,
            body: json!({
                "status": "error",
                "message": "No se pudieron publicar los pedidos en la cola interna.",
                "errors": errors
            })
            .to_string(),
            content_type: "application/json",
        };
    }

    let response_body = json!({
        "status": "queued",
        "queue": SHOPIFY_NEW_ORDER_QUEUE,
        "eventsPublished": published.len(),
        "published": published,
        "failed": errors
    })
    .to_string();

    EndpointResponse {
        status: 202,
        body: response_body,
        content_type: "application/json",
    }
}

async fn publish_order_event(
    client: &Client<HttpConnector>,
    publish_url: &str,
    order: &Order,
) -> Result<u16, String> {
    let payload = json!({
        "event": SHOPIFY_NEW_ORDER_EVENT,
        "queue": SHOPIFY_NEW_ORDER_QUEUE,
        "source": SERVICE_NAME,
        "order": {
            "id": order.id,
            "name": order.name,
            "createdAt": order.created_at,
            "totalPrice": order.total_price,
            "currency": order.currency,
            "financialStatus": order.financial_status,
            "fulfillmentStatus": order.fulfillment_status,
            "customer": {
                "name": order.customer_name,
                "email": order.customer_email
            },
            "tags": order.tags
        }
    })
    .to_string();

    let request = Request::builder()
        .method(Method::POST)
        .uri(publish_url)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(payload))
        .map_err(|error| error.to_string())?;

    let response = client
        .request(request)
        .await
        .map_err(|error| error.to_string())?;

    let status = response.status();
    let _ = body::to_bytes(response.into_body()).await;

    if status.is_success() {
        Ok(status.as_u16())
    } else {
        Err(format!("runner responded with HTTP {}", status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_endpoint_returns_health() {
        let response = dispatch_endpoint(&["health"]).await.expect("health endpoint");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_endpoint_returns_orders() {
        let response = dispatch_endpoint(&["shopify", "orders"])
            .await
            .expect("orders endpoint");
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert!(response.body.contains("\"orders\""));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn find_order_accepts_numeric_and_gid() {
        let order = find_order("1001").expect("order by numeric id");
        assert_eq!(order.id, "gid://shopify/Order/1001");

        let order_by_gid = find_order("gid://shopify/Order/0998").expect("order by gid");
        assert_eq!(order_by_gid.name, "#0998");
    }
}
