# Carpeta `src/`

Aquí vive el código fuente del runner principal. El archivo `main.rs` arranca los servicios
registrados, expone el endpoint de entrada y mantiene un sondeo periódico (`/health`) para cada
servicio configurado.

## Componentes clave

* **Carga de servicios**: se leen los manifiestos y la configuración JSON situada en
  `services/<nombre>/config/service.json`.
* **Arranque supervisado**: los servicios se levantan usando `cargo run`, se aplica el límite
  de memoria definido en la configuración y se espera a que el puerto quede accesible antes de
  continuar.
* **Proxy HTTP**: las peticiones entrantes se enrutan según el prefijo definido para cada
  servicio.
* **Panel web**: en `http://127.0.0.1:14000` se genera un resumen dinámico con el estado de
  salud (en línea, fuera de servicio o sin datos) y la marca de tiempo de la última verificación.
* **Endpoint de salud del runner**: el propio runner responde `200 OK` en `/health` para
  integrarse con herramientas externas.

## Comandos útiles

```bash
cargo fmt        # Formatear el código
cargo clippy     # Revisar linting
cargo run        # Ejecutar el runner
```
