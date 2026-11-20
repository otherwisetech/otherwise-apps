# catálogo
core.catalog.json = No se pudo leer el catálogo: {detail}
core.catalog.schema = El catálogo usa el formato {n}, que esta versión no entiende. Actualiza Otherwise Apps.
core.catalog.invalid = La entrada «{id}» del catálogo no es válida ({why}).
# archivos comprimidos
core.archive.io = No se pudo descomprimir la descarga: {detail}
core.archive.format = La descarga no es un archivo comprimido válido: {detail}
core.archive.unsafe = Se rechazó la descarga: «{path}» se escribiría fuera de la carpeta de la app.
core.archive.unsupported = Tipo de archivo desconocido: {name}
# instalación
core.install.io = No se pudo escribir {path}: {detail}
core.install.no_payload = La descarga no contiene la app donde se esperaba.
core.install.wrong_bundle = La descarga contiene otra app ({found} en lugar de {expected}); no se instaló nada.
core.install.not_writable = No hay permiso para instalar en {path}.
core.install.not_ours = {path} no es un lugar donde Otherwise Apps instale esta app; no se tocó.
core.install.trash = No se pudo mover {path} a la papelera: {detail}
# red
core.net.rate_limited = GitHub está limitando las solicitudes ahora; inténtalo de nuevo en unos minutos.
core.net.status = El servidor respondió con el error {code}.
core.net.transport = Error de red: {detail}
core.net.io = No se pudo guardar la descarga: {detail}
core.net.too_large = La descarga es más grande de lo esperado.
core.net.cancelled = Cancelado.
core.net.size = La descarga está incompleta ({got} de {want} bytes).
# verificación
core.verify.missing = GitHub no ha publicado una suma SHA-256 para este archivo, así que no se puede verificar y no se instaló.
core.verify.mismatch = La descarga no coincide con la suma SHA-256 de GitHub (se esperaba {expected}…, llegó {actual}…). Se borró y no se instaló nada.
