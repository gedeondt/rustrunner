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
  puede ser `bff`, `business` o `adapter`. Opcionalmente puede incluir un arreglo `schedules`
  con pares `endpoint` + `interval_secs` para programar webhooks.
* **Documentación OpenAPI**: cada servicio debe incluir un `openapi.json` sencillo con la lista de
  rutas que ofrece. El runner valida cada petición entrante contra esta definición antes de
  reenviarla al servicio correspondiente.
* **Compilación WebAssembly**: antes de ejecutar el runner es necesario compilar cada servicio a
  WebAssembly (WASI Preview 1). Puedes compilar todos los servicios de una sola vez con
  `./scripts/build_wasm_module.sh` o solo uno pasando su nombre como argumento. El runner carga el
  archivo `.wasm` generado en `services/<nombre>/target/wasm32-wasi/release/` y lo ejecuta con
  Wasmtime.

## Servicios incluidos

* `atencion_cliente_bff`: Backend for Frontend orientado a la app de clientes del banco.
* `atencion_cuenta_business`: servicio de negocio con el detalle de cuentas de cliente.
* `facturacion_sap_adapter`: adapter que aproxima la API de SAP para facturación y cobros.
