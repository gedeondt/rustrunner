# rustrunner

Este proyecto contiene un ejemplo mínimo de Rust y herramientas para comprobar que tu entorno tiene la versión adecuada del compilador.

## Requisitos previos

Asegúrate de tener instalado Rust en la versión `1.70.0` o superior. Puedes verificarlo con el script incluido:

```bash
./scripts/check_rust.sh
```

El script detecta automáticamente si estás en macOS o Linux y ofrece instrucciones de instalación o actualización usando `rustup`.

## Ejecutar el Hello World

Una vez verificado el entorno, compila y ejecuta el programa de ejemplo:

```bash
cargo run
```

Deberías ver el mensaje:

```
Hello, Rust!
```
