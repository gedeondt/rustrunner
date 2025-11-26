# wasmrunner

## Descripción general

**wasmrunner** es un runtime para middleware de integraciones construido en Rust. El objetivo
es ejecutar adaptadores y BFFs como módulos WebAssembly (WASI Preview 1), orquestarlos desde un
único punto de entrada y ofrecer un panel para entender qué ocurre con cada integración:
estado de salud, consumo de memoria, colas internas y ejecuciones programadas.

Cada pieza de integración se empaqueta como un módulo Wasm que wasmrunner carga mediante
**WasmEdge**. De esta forma obtenemos aislamiento ligero, arranques rápidos y la posibilidad de
delegar parte de la lógica de integración (enriquecimientos, transformaciones, llamadas HTTP) a
servicios independientes, ya sea un BFF, lógica de negocio o un adapter que conversa con un SaaS.
El repositorio también incluye utilidades para verificar el entorno antes de levantar el runtime.

## ¿Por qué un runtime para middleware?

* **Un solo entrypoint** para exponer BFFs, negocio y adapters sin replicar infraestructura.
* **Observabilidad integrada**: dashboard con salud, memoria, colas internas y webhooks.
* **Ejecución determinista**: los módulos Wasm encapsulan la lógica de integración y se pueden
  versionar/distribuir con el mismo contrato.
* **Automatización**: colas y webhooks permiten coordinar integraciones (sincronizaciones periódicas,
  disparo manual desde el dashboard o eventos entrantes).

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

2. Compila y ejecuta wasmrunner:

   ```bash
   cargo run
   ```

3. wasmrunner levantará automáticamente los módulos que encuentre en la carpeta `services/`
   y quedará escuchando en `http://127.0.0.1:14000`.

4. Abre la URL anterior en el navegador para ver el panel de resumen, donde se listan los
   servicios disponibles, su prefijo y el resultado del último sondeo de salud.

5. Cada servicio expone sus propios endpoints bajo su puerto correspondiente y un endpoint
   `GET /health` que responde con `200 OK`. wasmrunner consulta este endpoint cada cinco segundos
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

Además del sondeo de salud, cada servicio puede declarar webhooks que wasmrunner ejecutará de forma
periódica. Basta con añadir el bloque `schedules` en `config/service.json`, por ejemplo:

```json
{
  "prefix": "hello",
  "url": "http://127.0.0.1:15001",
  "memory_limit_mb": 64,
  "runners": 2,
  "schedules": [
    { "endpoint": "/hello", "interval_secs": 60 }
  ]
}
```

Cada entrada indica la ruta (relativa al servicio) y el intervalo de ejecución en segundos. El
panel de wasmrunner muestra todas las tareas programadas, el resultado HTTP de la última ejecución y
permite pausarlas o reanudarlas individualmente.

## Runners simultáneos y balanceo

El campo opcional `runners` dentro de `config/service.json` indica cuántas copias de un servicio
debe lanzar wasmrunner. A partir de la URL base se calcula un rango de puertos consecutivos, por lo
que `"url": "http://127.0.0.1:15001", "runners": 3` generará procesos en los puertos `15001`,
`15002` y `15003`. El reverse proxy interno reparte todas las peticiones HTTP del prefijo asignado
en round-robin entre estas copias y también ofrece controles para seguir pausando webhooks o lanzar
uno bajo demanda.

Cada servicio recibe variables de entorno adicionales:

* `WR_RUNNER_PORT`: puerto concreto asignado a la instancia.
* `WR_RUNNER_INDEX`: índice (0..N-1) dentro del grupo de runners.
* `WR_RUNNER_INSTANCES`: número total de copias configuradas.

Los servicios de ejemplo usan `WR_RUNNER_PORT` para ajustar el socket HTTP dinámicamente, pero
puedes reutilizar las otras variables para métricas o tareas internas si lo necesitas.

### Límite de memoria por servicio

El campo opcional `memory_limit_mb` establece la cuota máxima de memoria lineal que WasmEdge puede
asignar al módulo. wasmrunner convierte automáticamente ese valor a páginas WebAssembly (cada una de
64 KiB) y pasa `--memory-page-limit` al CLI cuando arranca el servicio —incluyendo ejecuciones
directas vía `cargo run -- --module <nombre>`. Por ejemplo, `64` equivale a `64 * 1024 / 64 = 1024`
páginas (≈64 MB). Si un servicio excede el límite configurado, WasmEdge lo terminará con un error.

## Estructura de carpetas

| Carpeta | Descripción |
|---------|-------------|
| `src/` | Código fuente del runtime y su API HTTP. |
| `services/` | Servicios de ejemplo que wasmrunner puede lanzar y monitorear. |
| `scripts/` | Utilidades para comprobar requisitos del entorno. |

Cada carpeta cuenta con un `README.md` adicional que profundiza en su contenido.

## Flujos habituales

* **Ver estado de los servicios**: visitar `http://127.0.0.1:14000` para revisar el resumen.
* **Consultar un servicio concreto**: acceder a wasmrunner con el prefijo definido en su
  configuración, por ejemplo `http://127.0.0.1:14000/hello/hello`.
* **Revisar el healthcheck de un servicio**: `curl http://127.0.0.1:15001/health` (o el puerto
  que corresponda).

## Contribuciones

Los cambios se trabajan mediante `cargo fmt` y `cargo clippy` para asegurar un estilo
consistente. Antes de abrir un PR, ejecuta también `cargo test` si añades pruebas.
