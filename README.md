# rustrunner

## Descripción general

Este repositorio contiene un "runner" escrito en Rust que inicia servicios HTTP de ejemplo,
los enruta mediante un punto de entrada único y expone un panel web con el estado de cada
servicio. También incluye utilidades para verificar el entorno de desarrollo requerido.

## Requisitos previos

* Rust 1.70.0 o superior (recomendado instalarlo mediante `rustup`).
* `cargo` disponible en la terminal.

Puedes comprobar la versión instalada ejecutando el script incluido:

```bash
./scripts/check_rust.sh
```

El script detecta automáticamente tu sistema operativo y te guía para actualizar Rust en caso
necesario.

## Puesta en marcha

1. Compila y ejecuta el runner principal:

   ```bash
   cargo run
   ```

2. El runner levantará automáticamente los servicios que encuentre en la carpeta `services/` y
   quedará escuchando en `http://127.0.0.1:14000`.

3. Abre la URL anterior en el navegador para ver el panel de resumen, donde se listan los
   servicios disponibles, su prefijo y el resultado del último sondeo de salud.

4. Cada servicio expone sus propios endpoints bajo su puerto correspondiente y un endpoint
   `GET /health` que responde con `200 OK`. El runner consulta este endpoint cada cinco segundos
   para actualizar el estado mostrado en el panel.

## Webhooks programados

Además del sondeo de salud, cada servicio puede declarar webhooks que el runner ejecutará de forma
periódica. Basta con añadir el bloque `schedules` en `config/service.json`, por ejemplo:

```json
{
  "prefix": "hello",
  "url": "http://127.0.0.1:15001",
  "memory_limit_mb": 64,
  "schedules": [
    { "endpoint": "/hello", "interval_secs": 60 }
  ]
}
```

Cada entrada indica la ruta (relativa al servicio) y el intervalo de ejecución en segundos. El
panel del runner muestra todas las tareas programadas, el resultado HTTP de la última ejecución y
permite pausarlas o reanudarlas individualmente.

## Estructura de carpetas

| Carpeta | Descripción |
|---------|-------------|
| `src/` | Código fuente del runner y su API HTTP. |
| `services/` | Servicios de ejemplo que el runner puede lanzar y monitorear. |
| `scripts/` | Utilidades para comprobar requisitos del entorno. |

Cada carpeta cuenta con un `README.md` adicional que profundiza en su contenido.

## Flujos habituales

* **Ver estado de los servicios**: visitar `http://127.0.0.1:14000` para revisar el resumen.
* **Consultar un servicio concreto**: acceder al runner con el prefijo definido en su
  configuración, por ejemplo `http://127.0.0.1:14000/hello/hello`.
* **Revisar el healthcheck de un servicio**: `curl http://127.0.0.1:15001/health` (o el puerto
  que corresponda).

## Contribuciones

Los cambios se trabajan mediante `cargo fmt` y `cargo clippy` para asegurar un estilo
consistente. Antes de abrir un PR, ejecuta también `cargo test` si añades pruebas.
