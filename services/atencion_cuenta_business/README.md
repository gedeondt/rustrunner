# Servicio `atencion_cuenta_business`

Servicio de negocio que centraliza la información de cuentas de clientes y expone los datos en
formato listo para consumo por otros sistemas de atención.

## Endpoints

| Método | Ruta                               | Descripción                                             |
|--------|------------------------------------|---------------------------------------------------------|
| GET    | `/health`                          | Healthcheck que responde `200 OK`.                      |
| GET    | `/accounts/{accountId}`            | Resumen principal de la cuenta.                         |
| GET    | `/accounts/{accountId}/holders`    | Lista de titulares y cotitulares vinculados.            |
| GET    | `/accounts/{accountId}/limits`     | Límite operativo diario y disponibilidad restante.      |
| GET    | `/accounts/{accountId}/movements`  | Últimos movimientos con importes y contrapartidas.      |

## Configuración

* `prefix`: `cuenta-cliente`. Se enruta como `http://127.0.0.1:14000/cuenta-cliente/...`.
* `url`: `http://127.0.0.1:15002`.
* `domain`: `atencion`. El dashboard lo agrupa con el resto de servicios del dominio.
* `type`: `business`. Identifica la capa funcional.

## Ejecución

```bash
cargo run --manifest-path services/atencion_cuenta_business/Cargo.toml
```
