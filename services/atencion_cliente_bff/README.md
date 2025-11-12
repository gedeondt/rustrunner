# Servicio `atencion_cliente_bff`

Backend for Frontend que simula la capa de experiencia para la aplicación móvil de un banco.
Ofrece endpoints agregados con datos dummy en castellano y un healthcheck compatible con el
runner.

## Endpoints

| Método | Ruta                               | Descripción                                          |
|--------|------------------------------------|------------------------------------------------------|
| GET    | `/health`                          | Responde `200 OK` con el estado básico del servicio. |
| GET    | `/clients/{clientId}/summary`      | Devuelve el resumen principal del cliente.           |
| GET    | `/clients/{clientId}/products`     | Lista productos activos y datos precalculados.       |
| GET    | `/clients/{clientId}/alerts`       | Muestra alertas operativas y de seguridad.           |
| GET    | `/clients/{clientId}/contact`      | Informa del gestor asignado y la oficina preferida.  |

## Configuración

El archivo `config/service.json` define:

* `prefix`: `cliente-bff`. La ruta pública será `http://127.0.0.1:14000/cliente-bff/...`.
* `url`: `http://127.0.0.1:15001`. Dirección y puerto donde escucha el servicio.
* `domain`: `atencion`. Permite agruparlo en el dashboard.
* `type`: `bff`. Identifica la tipología del servicio.

## Ejecución

```bash
cargo run --manifest-path services/atencion_cliente_bff/Cargo.toml
```
