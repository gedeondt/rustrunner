# rustrunner

## Descripción general

Este repositorio contiene un "runner" escrito en Rust que inicia servicios HTTP de ejemplo,
los enruta mediante un punto de entrada único y expone un panel web con el estado de cada
servicio. Cada servicio se distribuye como un módulo WebAssembly (WASI Preview 1) que el
runner carga y ejecuta usando **WasmEdge**, por lo que es necesario compilar dichos módulos
antes de arrancar el proceso principal y tener el runtime instalado. También se incluyen
utilidades para verificar el entorno de desarrollo requerido.

## Requisitos previos

* Rust 1.70.0 o superior (recomendado instalarlo mediante `rustup`).
* `cargo` disponible en la terminal.
* [WasmEdge](https://wasmedge.org/) 0.15 o superior instalado en el `PATH` (el script verificará
  la presencia del binario `wasmedge`).
* [wasi-sdk](https://github.com/WebAssembly/wasi-sdk) 24 (u otra versión compatible con
  `wasm32-wasip1`). Por convención lo instalamos en `~/.wasmedge/wasi-sdk-24.0`.

Puedes comprobar la versión instalada ejecutando el script incluido:

```bash
./scripts/check_rust.sh
```

El script detecta automáticamente tu sistema operativo y te guía para actualizar Rust en caso
necesario.

## Puesta en marcha

1. Compila los servicios a WebAssembly (se debe repetir cuando cambies código dentro de
   `services/`). El script detecta el wasi-sdk en `WASI_SDK_PATH` o en `~/.wasmedge/wasi-sdk-24.0`,
   exporta los compiladores requeridos y ejecuta `wasmedge compile` para generar los binarios AoT:

   ```bash
   ./scripts/build_wasm_module.sh
   ```

2. Compila y ejecuta el runner principal:

   ```bash
   cargo run
   ```

3. El runner levantará automáticamente los módulos que encuentre en la carpeta `services/`
   y quedará escuchando en `http://127.0.0.1:14000`.

4. Abre la URL anterior en el navegador para ver el panel de resumen, donde se listan los
   servicios disponibles, su prefijo y el resultado del último sondeo de salud.

5. Cada servicio expone sus propios endpoints bajo su puerto correspondiente y un endpoint
   `GET /health` que responde con `200 OK`. El runner consulta este endpoint cada cinco segundos
   para actualizar el estado mostrado en el panel.

## Pila HTTP obligatoria

Todos los servicios deben utilizar las bibliotecas recomendadas por WasmEdge tanto para el
servidor como para los clientes HTTP. Esto garantiza que las llamadas funcionen sobre el
soporte de sockets proporcionado por el runtime.

* **Servidor HTTP:** `hyper` + `tokio` con las mismas `features` empleadas en los servicios
  de ejemplo. Además, cada servicio debe incluir los siguientes parches en su `Cargo.toml`:

  ```toml
  [patch.crates-io]
  tokio = { git = "https://github.com/second-state/wasi_tokio.git", branch = "v1.36.x" }
  socket2 = { git = "https://github.com/second-state/socket2.git", branch = "v0.5.x" }
  hyper = { git = "https://github.com/second-state/wasi_hyper.git", branch = "v0.14.x" }
  ```

* **Cliente HTTP:** `reqwest` con TLS basado en rustls más el parche oficial para WasmEdge:

  ```toml
  reqwest = { version = "0.11", default-features = false, features = ["rustls-tls"] }

  [patch.crates-io]
  reqwest = { git = "https://github.com/second-state/wasi_reqwest.git", branch = "0.11.x" }
  ```

No se permite ninguna otra librería o cliente HTTP (por ejemplo `ureq`, `surf`, `isahc`, etc.)
ni variantes de `reqwest`/`hyper` sin los parches anteriores. Todos los agentes y servicios deben
respetar esta convención para evitar incompatibilidades con WasmEdge.

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

### Límite de memoria por servicio

El campo opcional `memory_limit_mb` establece la cuota máxima de memoria lineal que WasmEdge puede
asignar al módulo. El runner convierte automáticamente ese valor a páginas WebAssembly (cada una de
64 KiB) y pasa `--memory-page-limit` al CLI cuando arranca el servicio —incluyendo ejecuciones
directas vía `cargo run -- --module <nombre>`. Por ejemplo, `64` equivale a `64 * 1024 / 64 = 1024`
páginas (≈64 MB). Si un servicio excede el límite configurado, WasmEdge lo terminará con un error.

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
