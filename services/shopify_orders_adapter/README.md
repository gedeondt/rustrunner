# Servicio `shopify_orders_adapter`

Adapter que envuelve los endpoints de pedidos de Shopify y publica eventos de pedidos nuevos en la
cola interna del runner. Simula respuestas del API de Shopify y permite lanzar un webhook que
emite un evento `ShopifyOrderCreated` por cada pedido detectado.

## Endpoints

| Método | Ruta                           | Descripción                                                    |
|--------|--------------------------------|----------------------------------------------------------------|
| GET    | `/health`                      | Healthcheck estándar.                                          |
| GET    | `/shopify/orders`              | Lista pedidos recientes con datos principales.                 |
| GET    | `/shopify/orders/{orderId}`    | Devuelve el detalle de un pedido concreto.                     |
| GET    | `/webhooks/orders/pull`        | Simula una sincronización y publica eventos de pedidos nuevos. |

## Configuración

* `prefix`: `shopify-adapter`. Se expone como `http://127.0.0.1:14000/shopify-adapter/...`.
* `url`: `http://127.0.0.1:15004`.
* `domain`: `ecommerce`.
* `type`: `adapter`.
* `schedules`: ejecuta `/webhooks/orders/pull` cada 120 segundos para emitir eventos nuevos.

## Ejecución

```bash
cargo run --manifest-path services/shopify_orders_adapter/Cargo.toml
```
