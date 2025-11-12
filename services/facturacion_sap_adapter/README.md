# Servicio `facturacion_sap_adapter`

Adapter que simula la integración con SAP para consultar deuda, facturas abiertas y contactos
operativos de un cliente corporativo.

## Endpoints

| Método | Ruta                                          | Descripción                                          |
|--------|-----------------------------------------------|------------------------------------------------------|
| GET    | `/health`                                     | Healthcheck estándar.                                |
| GET    | `/sap/customers/{customerId}/debt`            | Devuelve el resumen de deuda calculado por SAP.      |
| GET    | `/sap/customers/{customerId}/invoices`        | Lista las facturas abiertas y su estado.             |
| GET    | `/sap/customers/{customerId}/status`          | Informa del riesgo y estado de bloqueo.              |
| GET    | `/sap/customers/{customerId}/contacts`        | Datos de contacto para coordinar gestiones de cobro. |

## Configuración

* `prefix`: `sap-adapter`. Se publica como `http://127.0.0.1:14000/sap-adapter/...`.
* `url`: `http://127.0.0.1:15003`.
* `domain`: `facturacion`. El dashboard lo agrupa por dominio.
* `type`: `adapter`. Identifica la capa técnica.

## Ejecución

```bash
cargo run --manifest-path services/facturacion_sap_adapter/Cargo.toml
```
