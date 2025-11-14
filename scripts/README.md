# Carpeta `scripts/`

Contiene utilidades auxiliares para preparar el entorno de desarrollo.

## `check_rust.sh`

* Detecta el sistema operativo (macOS o Linux).
* Comprueba la versión instalada de Rust.
* Sugiere la instalación o actualización mediante `rustup` en caso de ser necesario.

Ejecuta el script desde la raíz del repositorio:

```bash
./scripts/check_rust.sh
```

## `check_wasm_toolchain.sh`

* Comprueba que el objetivo `wasm32-wasip2` esté instalado en `rustup`.
* Verifica que `cargo` esté disponible antes de construir los servicios como WebAssembly.

Úsalo cuando configures el entorno por primera vez:

```bash
./scripts/check_wasm_toolchain.sh
```

## `build_wasm_module.sh`

* Ejecuta el script de la carpeta `services/` que compila cada servicio hacia `wasm32-wasip2` en
  modo `release`.
* Acepta un nombre de servicio para compilar uno en concreto o se encarga de todos si no se le
  pasa argumento.

Este paso es obligatorio antes de lanzar el runner:

```bash
./scripts/build_wasm_module.sh
```
