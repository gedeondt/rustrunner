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
* **Configuración**: el archivo `config/service.json` define el prefijo de enrutamiento, la URL
  base (incluyendo el puerto) y el límite de memoria (en MiB) que el runner aplica al proceso.
  Opcionalmente puede incluir un arreglo `schedules` con pares `endpoint` + `interval_secs` para
  que el runner invoque webhooks de forma periódica.
* **Documentación OpenAPI**: cada servicio debe incluir un `openapi.json` sencillo con la lista de
  rutas que ofrece. El runner valida cada petición entrante contra esta definición antes de
  reenviarla al servicio correspondiente.
* **Ejecución local**: cada servicio puede iniciarse de forma independiente con `cargo run` desde
  su carpeta, aunque el runner se encarga de hacerlo automáticamente cuando está disponible.

Los servicios incluidos (`hello_world` y `bye_world`) son ejemplos simples pensados para mostrar
cómo extender el catálogo.
