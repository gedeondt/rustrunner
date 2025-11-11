# Carpeta `services/`

La carpeta agrupa los servicios HTTP de ejemplo que el runner puede administrar. Cada servicio
sigue la misma estructura:

```
services/
  <nombre>/
    Cargo.toml
    src/main.rs
    config/service.json
```

## Convenciones

* **Endpoint de salud**: todos los servicios deben exponer `GET /health` y responder siempre
  con `200 OK`. El runner consulta esta ruta cada cinco segundos para actualizar el panel.
* **Configuración**: el archivo `config/service.json` define el prefijo de enrutamiento y la URL
  base (incluyendo el puerto).
* **Ejecución local**: cada servicio puede iniciarse de forma independiente con `cargo run` desde
  su carpeta, aunque el runner se encarga de hacerlo automáticamente cuando está disponible.

Los servicios incluidos (`hello_world` y `bye_world`) son ejemplos simples pensados para mostrar
cómo extender el catálogo.
