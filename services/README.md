# Carpeta `services/`

La carpeta agrupa los servicios HTTP de ejemplo que el runner puede administrar. Cada servicio
sigue la misma estructura:

```
services/
  <nombre>/
    Cargo.toml
    src/main.rs
    config/service.json
    openapi.json
```

## Convenciones

* **Endpoint de salud**: todos los servicios deben exponer `GET /health` y responder siempre
  con `200 OK`. El runner consulta esta ruta cada cinco segundos para actualizar el panel.
* **Configuración**: el archivo `config/service.json` define el prefijo de enrutamiento,
  la URL base (incluyendo el puerto), el dominio lógico (`domain`) y la tipología (`type`) que
  puede ser `bff`, `business` o `adapter`. El campo opcional `runners` indica cuántas copias
  simultáneas levantará wasmrunner. El runtime reutiliza la URL base como puerto inicial y asigna
  los siguientes puertos de forma incremental (`15001`, `15002`, …). Además acepta `memory_limit_mb`
  para fijar el límite de memoria asignado al módulo y un arreglo `schedules` con pares
  `endpoint` + `interval_secs` para programar webhooks.
* **Documentación OpenAPI**: cada servicio debe incluir un `openapi.json` sencillo con la lista de
  rutas que ofrece. El runner valida cada petición entrante contra esta definición antes de
  reenviarla al servicio correspondiente.
* **Compilación WebAssembly**: antes de ejecutar el runner es necesario compilar cada servicio a
  WebAssembly (WASI Preview 1). Puedes compilar todos los servicios de una sola vez con
  `./scripts/build_wasm_module.sh` o solo uno pasando su nombre como argumento. El script configura
  automáticamente el wasi-sdk y usa `wasmedge compile` para generar `services/<nombre>/<nombre>.wasm`,
  que es el artefacto que el runner arranca mediante la CLI de WasmEdge.

### Pila HTTP obligatoria

Para garantizar compatibilidad con WasmEdge **todos los servicios deben usar exactamente las
dependencias siguientes** para servidores y clientes HTTP:

```toml
[dependencies]
hyper = { version = "0.14", features = ["http1", "server", "runtime"] }
tokio = { version = "1", features = ["macros", "rt", "net", "io-util", "time"] }
reqwest = { version = "0.11", default-features = false, features = ["rustls-tls"] }

[patch.crates-io]
tokio = { git = "https://github.com/second-state/wasi_tokio.git", branch = "v1.36.x" }
socket2 = { git = "https://github.com/second-state/socket2.git", branch = "v0.5.x" }
hyper = { git = "https://github.com/second-state/wasi_hyper.git", branch = "v0.14.x" }
reqwest = { git = "https://github.com/second-state/wasi_reqwest.git", branch = "0.11.x" }
```

No se admite ninguna otra librería (`ureq`, `surf`, variantes sin parches, etc.). Cada agente y
servicio debe realizar las peticiones salientes únicamente con `reqwest` y exponer los endpoints
HTTP usando `hyper` + `tokio`.

## Variables de entorno inyectadas

wasmrunner expone varias variables a cada servicio para coordinar las copias:

* `WR_RUNNER_PORT`: puerto asignado al proceso actual.
* `WR_RUNNER_INDEX`: índice de la réplica (empezando en 0).
* `WR_RUNNER_INSTANCES`: número total de copias configuradas para el servicio.

Los servicios de ejemplo ya leen `WR_RUNNER_PORT` para ajustar el `bind_addr`. Si tu integración
necesita un comportamiento especial por réplica (p. ej. métricas), puedes consultar también
`WR_RUNNER_INDEX` y `WR_RUNNER_INSTANCES`.

## Servicios incluidos

* `atencion_cliente_bff`: Backend for Frontend orientado a la app de clientes del banco.
* `atencion_cuenta_business`: servicio de negocio con el detalle de cuentas de cliente.
* `facturacion_sap_adapter`: adapter que aproxima la API de SAP para facturación y cobros.

## Límites de memoria

WasmEdge implementa la opción `--memory-page-limit`, donde cada página equivale a 64 KiB. Para no
exponer este detalle en cada config, el runner acepta el campo `memory_limit_mb` y calcula la cuota
en páginas antes de lanzar el proceso (tanto en el modo panel como en `cargo run -- --module ...`),
aplicando el límite a través de la CLI. Si no defines el campo, el módulo se ejecutará sin límite
explícito (queda sujeto a lo que el host permita). Un valor de `100` MB, por ejemplo, se transforma
en `100 * 16 = 1600` páginas, por lo que no se permitirá que el servicio crezca por encima de
~102 MB.
