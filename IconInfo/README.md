# IconInfo · Logo de Swiftbeat

Kit completo del logo «S calada»: un play con la S recortada dentro. Las reglas de uso están en [GUIA-DE-MARCA.md](GUIA-DE-MARCA.md).

<p align="center"><img src="png/swiftbeat-horizontal-color-1600.png" width="480" alt="Logo de Swiftbeat"></p>

## Contenido

| Carpeta | Qué hay |
|---|---|
| `svg/` | Maestros vectoriales: símbolo, corte pequeño, horizontal, apilado y palabra, cada uno en color, negro y blanco. `swiftbeat-icon*.svg` es el símbolo recortado al cuadrado para iconos; `swiftbeat-app-icon.svg`, el símbolo blanco sobre una baldosa lavanda. |
| `png/` | Exportaciones en alta resolución con fondo transparente. |
| `windows/` | `swiftbeat.ico` (16–256 px; de 16 a 32 px usa el corte pequeño) y `swiftbeat.png` (256 px). Tienen los mismos nombres que los de `assets/`. |
| `web/` | `favicon.ico`/`.svg`, iconos PNG, `site.webmanifest` y `head-snippet.html` con las etiquetas `<head>` listas para pegar. |
| `presentacion/` | `presentacion.html` (autocontenida) y `diapositivas/` en PNG, con mockups de uso. |
| `fuente/` | Scripts que generan todos los SVG (`build_kit.py` + `gen_s.py`). |
| `licencias/` | Licencia OFL de la fuente Inter. |

## Usarlo en la app

Para cambiar el icono del ejecutable y del instalador, sustituye `assets/swiftbeat.ico` y `assets/swiftbeat.png` por los de `windows/` y vuelve a compilar.

Desde la 0.1.3 ya están copiados, y `installer/make-icon.ps1` hace esa copia si los vuelves a exportar. La interfaz dibuja el símbolo y la palabra como vectores en `ui/brand.slint`, con los mismos trazados que `svg/`.

## Regenerar los SVG

Requisitos: Python 3 con `fonttools` y el archivo variable de Inter (`Inter[opsz,wght].ttf`, de [Google Fonts](https://github.com/google/fonts/tree/main/ofl/inter)).

```powershell
cd IconInfo\fuente
python build_kit.py ..\svg "ruta\a\Inter[opsz,wght].ttf"
```

La geometría se define en código: el eje de la S está en `gen_s.py` (`SEGS`) y la escala, el grosor y la posición de la S dentro del triángulo, en las llamadas a `symbol_d()` de `build_kit.py`. Los PNG, el `.ico` y el set web se exportaron con los scripts de la skill `logo-design`.
