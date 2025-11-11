# Servicio `bye_world`

Servicio hermano de `hello_world` que reutiliza la misma estructura de endpoints, pero responde
identificándose como `bye`.

## Endpoints

| Método | Ruta         | Descripción                                   |
|--------|--------------|-----------------------------------------------|
| GET    | `/hello`     | Devuelve un saludo genérico indicando la identidad del servicio. |
| GET    | `/bye`       | Respuesta con un mensaje de despedida.        |
| GET    | `/health`    | Devuelve `ok` y estado `200` para comprobaciones de vida. |

## Configuración

`config/service.json` establece:

* `prefix`: `bye`. El runner expone sus rutas en `http://127.0.0.1:14000/bye/<endpoint>`.
* `url`: `http://127.0.0.1:15002`, puerto local donde escucha el binario.

## Ejecución

```bash
cargo run --manifest-path services/bye_world/Cargo.toml
```

El runner también se encarga de iniciarlo automáticamente al arrancar.
