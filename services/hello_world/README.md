# Servicio `hello_world`

Servicio de ejemplo que responde con mensajes sencillos en castellano. Está pensado para mostrar
cómo registrar un servicio en el runner y cómo exponer un healthcheck compatible.

## Endpoints

| Método | Ruta         | Descripción                                   |
|--------|--------------|-----------------------------------------------|
| GET    | `/hello`     | Devuelve un saludo con la identidad del servicio. |
| GET    | `/bye`       | Muestra un mensaje de despedida.              |
| GET    | `/health`    | Responde siempre `200 OK` con cuerpo `ok`.    |

## Configuración

El archivo `config/service.json` define:

* `prefix`: `hello`. Así accedes al servicio a través del runner mediante
  `http://127.0.0.1:14000/hello/<endpoint>`.
* `url`: `http://127.0.0.1:15001`. Es la dirección donde escucha el servicio de forma directa.
* `memory_limit_mb`: `64`. El runner fija un máximo de 64 MiB para el proceso cuando lo inicia.

## Ejecución

```bash
cargo run --manifest-path services/hello_world/Cargo.toml
```

El runner lanzará este servicio automáticamente si está incluido en la carpeta `services/`.
