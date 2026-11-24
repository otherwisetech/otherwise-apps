# Servidor Otherwise: las apps de escritorio contra un servidor (Local o Mac Studio)

Estado: **aprobado e implementándose** (fases 0-2 en expense Business, piloto en el Mac Studio). Las decisiones del usuario y lo que se construyó están en §14. Fecha: 2026-11-24.

La idea, ya aprobada en principio, es que las apps GPUI (expense Business, expense me, Brújula, Vigía,
Nodo, genealogy, Mi Huerta, Surco, Voltia, Helio, CACFU) puedan trabajar contra un **servidor**,
como lo hace OpenBot. El usuario elige **Local** o un servidor remoto, por ejemplo el Mac Studio del
tailnet. Así el usuario, su hermano y las futuras apps móviles trabajan sobre los mismos datos, y las
apps siguen corriendo sin ventana en el Studio todo el tiempo.

Este documento valida contra el código la arquitectura que propuso el coordinador y la ajusta donde
el código sugiere otra cosa. Las referencias son a los repos en `~/` en la fecha de arriba.

---

## 0. Resumen de la recomendación

1. **Un endpoint por máquina:** Otherwise Helper (`crates/helper`, sin GPUI) gana un modo servidor.
   Escucha solo en loopback o en un socket Unix y se publica en el tailnet con `tailscale serve`
   (HTTPS). Lanza procesos `<app> serve` sin ventana bajo demanda y los detiene cuando quedan
   inactivos, con el mismo `runner` (timeout, techo de memoria, `nice`) que ya usa para los jobs.
2. **El servidor es el único escritor.** Esto ya existe en expense: `core/src/appsock.rs` hace que
   la CLI y el MCP escriban *a través* de la app abierta (`Run`, `Undo`, `Propose`, `revision`).
   El servidor HTTP es ese mismo protocolo, sacado a la red con identidad.
3. **Ajuste a la propuesta: "Local" no pasa por HTTP en localhost.** Las apps tienen **una sola
   API**: un trait `Backend` con operaciones, consultas y suscripción. Tiene dos implementaciones:
   - `LocalBackend` (en proceso, el código de hoy);
   - `RemoteBackend` (HTTP).

   Se gana lo mismo que con "un solo camino de código", pero sin depender del Helper para abrir un
   libro local, sin puertos y sin la latencia de un salto HTTP en cada lectura. Windows, que todavía
   no tiene appsock, tampoco queda atrás.
4. **Lecturas remotas por réplica, escrituras por operaciones.** Para expense (Fase 1) el servidor
   entrega una **instantánea SQLite de solo lectura** (`VACUUM INTO`), más eventos `changed`. El
   cliente la abre con `Library::open_read_only`, de modo que **todas las vistas actuales funcionan
   sin reescribir las consultas**. Las escrituras viajan como `ops::OPS` por nombre, igual que
   `appsock::Request::Run`. Así no hace falta sacar a las apps de SQLite directo: solo dejan de
   *escribirlo* cuando el libro es remoto.
5. **Brújula sincroniza su op log** (`ops`, HLC, LWW, `unsynced()`/`merge()`), que ya está listo
   para esto (`docs/ARCHITECTURE.md:71-72`). El servidor filtra las operaciones por rol y módulo y
   asigna el cursor.
6. **Identidad = Tailscale** (cabeceras de `tailscale serve` y `whois` del LocalAPI), mapeada a
   miembros y roles por app y libro. Nada se expone a internet (no se usa Funnel).
7. **Los secretos no salen del Studio.** Los importadores (IMAP `expense-imap`, PDF
   `expense-statement-pdf`) corren como jobs del Helper en el servidor, con el keychain del
   servidor.
8. **UI:** un `ServerSwitcher` en otherwise-ui, en el slot `Sidebar::header`. El registro de
   servidores, el descubrimiento y el cliente viven en otherwise-link (crate nuevo
   `otherwise-remote`).
9. **El contrato HTTP es el mismo que implementará la sincronización con Vcity:** hello,
   instantánea, ops, cursor y eventos. Al cliente no le importa si habla con el Studio o con Vcity.
10. **Licencias por tamaño de equipo** (revisado tras la revisión del usuario):
    - la licencia es del equipo y fija un máximo de miembros activos;
    - el servidor cuenta las identidades distintas en 30 días;
    - el servidor informa a Vcity solo conteos anónimos;
    - la aplicación es suave: aviso, gracia y luego solo lectura para los miembros de más (§10.2).

    Primero va expense Business, luego Brújula y luego Vigía (§12).
11. **Riesgo principal detectado en el Studio:** los 3 runners de CI corren como **el mismo usuario
    de macOS** que tendría el servidor. Por eso "viene de localhost" no puede significar "es de
    confianza" (ver §6.1).

---

## 1. Objetivos y no-objetivos

### Objetivos

- **Elegir el servidor:** Local o un servidor del tailnet, por app, desde la barra lateral.
- **Datos compartidos:** varias personas y dispositivos (usuario, hermano, móviles) trabajan sobre
  los mismos datos, **sin** el intercambio de archivos por OneDrive y sin el aviso "el libro está
  abierto en otro computador".
- **Apps siempre activas:** las apps corren sin ventana en el Studio de forma continua (correo,
  recordatorios, monitor de red, pronóstico), en un solo lugar y sin depender de qué portátil está
  abierto.
- **Mismo modelo de seguridad que hoy:** escrituras de agentes solo con permiso, secretos en el
  keychain y nada escuchando en internet.
- **Un contrato de API** que sirva igual para el Studio, para un on-prem de un cliente y para Vcity
  cloud.
- **Pasos pequeños** que se puedan probar, empezando por algo útil de inmediato: ver el libro de
  expense del Studio desde el portátil.

### No-objetivos (por ahora)

- **Internet público:** nada de Funnel, puertos abiertos ni cuentas propias. El acceso fuera del
  tailnet es Vcity cloud, una fase aparte.
- **Edición colaborativa en tiempo real** de textos largos (CRDT de texto, Yjs): Brújula lo deja
  para "más adelante" (`ARCHITECTURE.md:108`).
- **Convertir las apps de cálculo por documento** (Surco, Voltia, Helio, CACFU) en multiusuario.
  Como mucho, abrir y guardar un documento remoto con control de revisión.
- **Escrituras offline en expense en las primeras fases.** Sin conexión, el cliente queda en solo
  lectura con la última instantánea.
- **Reemplazar el data root ni la reubicación de otherwise-link:** Local sigue siendo exactamente lo
  de hoy.
- **Cobro, tienda y precios de los tramos:** son de Vcity. Aquí solo se define qué cuenta y comprueba el
  servidor (§10.2).

---

## 2. Arquitectura

```
                    tailnet (WireGuard; ACLs de Tailscale)
 ┌──────────────────────────┐                   ┌───────────────────────────────────────────────┐
 │ MacBook / iMac / Windows │                   │ Mac Studio (siempre encendido)                │
 │                          │  HTTPS            │                                               │
 │ expense.app ─────────────┼──────────────────▶│ tailscale serve :443                          │
 │  ├ ServerSwitcher (ui)   │  /v1/...          │   añade Tailscale-User-Login/-Name            │
 │  ├ Backend               │  + SSE /events    │   proxy → 127.0.0.1:47900 (o unix socket)     │
 │  │  ├ LocalBackend ──────┼─┐                 │        │                                      │
 │  │  └ RemoteBackend      │ │                 │        ▼                                      │
 │  └ caché de réplica      │ │                 │ Otherwise Helper (modo servidor, sin GPUI)    │
 │    (device dir)          │ │                 │  ├ identidad: cabeceras + whois LocalAPI      │
 │                          │ │                 │  ├ miembros/roles · auditoría · límites       │
 │ Brújula.app ─────────────┼─┼───────────────▶ │  ├ router /v1/apps/<app>/...                  │
 │ Vigía.app …              │ │                 │  ├ supervisor: lanza `<app> serve`,           │
 │                          │ │                 │  │   para al quedar inactivo                  │
 │ datos locales (como hoy) │◀┘                 │  └ jobs de siempre (otherwise-background.json)│
 │ ~/.otherwise · OneDrive  │                   │        │ stdio / unix socket (JSON por línea) │
 └──────────────────────────┘                   │        ▼                                      │
                                                │ expense serve --book …  (único escritor)      │
 ┌──────────────────────────┐                   │ brujula serve --workspace …                   │
 │ iPhone / Android         │  HTTPS (app       │ vigia serve …                                 │
 │ (Tailscale)              ├──────────────────▶│        │                                      │
 │  cliente HTTP del mismo  │  Tailscale)       │        ▼                                      │
 │  contrato                │                   │ ~/.otherwise (disco local, NO OneDrive)       │
 └──────────────────────────┘                   │ keychain del Studio: expense-imap, PDFs       │
                                                │ copias nocturnas → backups/                   │
            mismo contrato /v1                  └───────────────────────────────────────────────┘
 ┌──────────────────────────┐
 │ Vcity cloud (futuro)     │◀── RemoteBackend con servidor de tipo "vcity" (auth = sesión Vcity)
 └──────────────────────────┘
```

**Capas y repos:**

| Pieza | Repo y crate | Hace |
|---|---|---|
| `ServerSwitcher` (vista) | `~/otherwise-ui` `components/servers.rs` (nuevo) | Tile, menú y estados. Recibe datos y callbacks; no hace red. |
| Registro, descubrimiento y cliente | `~/otherwise-link`: crate nuevo `crates/remote` (`otherwise-remote`), más tipos en `otherwise-link::server` | `servers.json` en `paths::device_dir()`, sondeo de pares del tailnet, cliente HTTP+SSE, caché de réplicas. |
| Contrato | `~/otherwise-link/docs/SPEC.md` §12 (nuevo) y tipos serde compartidos | Mensajes `hello`, `snapshot`, `ops`, `events` y errores. |
| Servidor | `~/otherwise-apps` `crates/server` (nuevo, lib sin GPUI), enlazado por `otherwise-helper` | HTTP, identidad, roles, auditoría, supervisor y router. |
| Núcleo de cada app | `expense-core`, `brujula-core`, `vigia-core`… | `<app> serve`: el protocolo de appsock por stdio o socket. |

El Helper **sigue sin enlazar código de ninguna app** (regla del `CLAUDE.md` del hub): solo lanza
el binario verificado de cada app, como ya hace con los jobs (`background::plan`). Así se pueden
actualizar las apps sin tocar el Helper, y una app solo se expone si está instalada en el servidor.

---

## 3. Qué existe hoy, por app

### 3.1 Piezas compartidas

**Otherwise Helper** (`~/otherwise-apps/crates/helper`):
- `engine.rs`: escaneo e instalación verificada antes de cada ejecución.
- `sched.rs`: pausa, Low Power, presión de memoria, sin red, backoff.
- `runner.rs`: binario propio sin shell, `setpriority(10)`, grupo de procesos, timeout y techo de
  RSS.
- `state.rs`: estado en `paths::device_dir()`.
- `mac.rs`: `NSStatusItem` y `SMAppService`.

Ya es un supervisor de procesos de apps. Le falta un servidor de red y procesos de larga duración
(hoy todo es corrida → salida).

**otherwise-link** (`~/otherwise-link`, v0.5.3):
- `lock.rs`: `<file>.lock.json` con heartbeat.
- `cloud.rs`: detección de OneDrive y otras nubes, y de copias en conflicto.
- `background.rs`: jobs, lease `background.lock.json` y outbox.
- `paths.rs`: `device_dir()`.
- `registry.rs`: entidades con `changes.seq` y cursores, un modelo pull/ack que ya se parece al que
  necesitamos (SPEC §4.4).
- `crates/hub`: proxy MCP stdio de los hijos `<cli> mcp`.

SPEC §11.2 ya exige el único escritor: *"a job that writes MUST send its writes through the app
when the app has the data open (its local socket)"*. SPEC §2.2 dice: *"Real multi-device sync is a
separate, future layer (`otherwise-sync`)"*. Este plan es esa capa.

**otherwise-ui** (`~/otherwise-ui`, v0.1.0): `Sidebar` en `src/components/chrome.rs:126`, con el
slot `header(el)`, documentado como *"Any element at the top instead of the brand (a workspace
switcher)"*. No hay un slot llamado `switcher`: el lugar es `Sidebar::header`. Hoy **solo el hub**
usa otherwise-ui (#16). expense (`app/src/theme.rs:2-3`), expense me, Brújula (`SIDEBAR_W`,
`app.rs:27`) y el resto tienen barra lateral propia.

**Studio:** no tiene ninguna app Otherwise instalada (`/Applications` solo muestra Docker, Ollama,
Tailscale y OpenBot), ni `tailscale serve` configurado. Ver §8.

### 3.2 Por app

| App | Núcleo sin UI | Almacenamiento | Lock | MCP (stdio) | CLI sin ventana / jobs | Escritor único hoy |
|---|---|---|---|---|---|---|
| **expense Business** `~/expense-rs` v0.9.3 | `crates/core` (expense-core) y `crates/cli` (bin `expense`, "runs on servers and in containers", `docs/cli.md:6`) | Carpeta del libro elegida por el usuario (a menudo OneDrive): `expense.db` (DELETE journal, `synchronous=FULL`), `expense-backup.json`, `meta.revision` | `core/src/lock.rs`: `expense.lock`, heartbeat 60 s, stale a 360 s; otro computador → solo lectura | `core/src/mcp.rs`: ~53 lecturas (`ai_tools::TOOLS`), propose_*; escrituras `ops::OPS` (96 ops, `ops.rs:158`) con `--allow-writes` + `agent_writes` | Muchos subcomandos (`cli/src/lib.rs:43-110`), **sin `serve`**; jobs mail, reminders, inbox (`core/src/background.rs`) | **Sí:** `core/src/appsock.rs` (Hello, Run, Undo, Propose, revision; `docs/app-socket.md`; planeados `subscribe` y `tools`). Unix, peer-uid, sin Windows. `phone.rs`: HTTP LAN con token, solo subidas |
| **expense me** `~/expense-me` v0.3.5 | `crates/me` sobre expense-core `v0.9.1` (`personal`) | Igual que Business, en `<data root>/expense/<nombre>` | Igual (`expense.lock`) | `me/src/mcp.rs`, filtrado por `me::allow` | Solo `background` y `mcp` | Igual que Business (appsock de expense-core) |
| **Brújula** `~/brujula-rs` v0.3.0 | `crates/core` (brujula-core) | `<Empresa>.brujula` SQLite: tabla **`ops` de solo anexado** (triggers prohíben DELETE/UPDATE salvo `synced`), `records` derivado, HLC (`hlc.rs`), LWW por (registro, campo), ids uuid v4/v5; actor = usuario×dispositivo | otherwise-link `Lock` proceso (`app/src/storage_ui.rs`) | `core/src/mcp.rs`: 10 lecturas, 6 propose_* a buzón | Solo `mcp`; **costura de sync** `ops()`/`unsynced()`/`merge()` (`store.rs:482-520`) | Sí, por proceso; la costura de sync está lista para un servidor |
| **Vigía** `~/vigia-rs` v0.6.2 | `crates/core` y `crates/cli` (bin `vigia`) | `vigia.db` SQLite con tablas mutables, sin op log | `<db>.lock.json` (`app/src/state.rs`) | `cli/src/mcp.rs`: 9 lecturas; add_item, renew_item, propose | CLI completa (`list…sync`, `--json`); jobs reminders (15 min) y checks (1 h: RDAP, TLS) | Los jobs no escriben si la app tiene el lock; **sin socket** |
| **Nodo** `~/nodo-rs` | core y cli | Un SQLite por mapa (`<data root>/nodo`) | `<map>.lock.json` | `cli/src/mcp.rs`: 7 lecturas; update_device, add_service (directo) | Job `monitor` cada 5 min → `background.json`; compartir P2P en TCP 47821 | Parcial |
| **Mi Huerta** `~/mi-huerta` | `huerta` (portable para móvil) y cli | `garden.json` con `revision`; los guardados rechazan una revisión más nueva | `garden.json.lock.json` | 4 lecturas, 6 propose_* | Jobs `reminders` (lectura) y `forecast` (escribe con lock) | CAS de documento |
| **genealogy** `~/genealogy-rs` | `gedcom` y `mcp` | El `.ged` del usuario | `.lock.json` | 9+ lecturas, propose_* (`--propose`) | Solo `mcp` | Documento |
| **Surco** `~/surco-rs` | `*-core` por herramienta, `finca` | JSON por proyecto y `.sfinca` | `Lock` por proyecto | Sobre una instantánea `$TMPDIR/surco-agent-<pid>` | Ninguno | Documento |
| **Voltia** `~/voltia-rs` | Igual que Surco | `.vobra` | Lock, "Usar aquí de todos modos" | Igual que Surco | Ninguno | Documento |
| **Helio** `~/helio` | core | `.helio` JSON | Lock 1 min | **No** | Ninguno | Documento |
| **CACFU** `~/cacfu-rs` | core | `.cacfu` JSON | Lock | No en main (sí en `feat/agent-panel`) | Ninguno | Documento |

Versiones de otherwise-link en uso: hub 0.5.3, expense me 0.5.2, Mi Huerta y Surco 0.5.1, Vigía y
Nodo 0.5.0, expense 0.4.0, Brújula, genealogy y Helio 0.3.0, CACFU 0.2.0. Cualquier pieza nueva en
otherwise-link exige subir de versión app por app.

### 3.3 Brechas

1. **Ninguna app tiene `serve`.** expense tiene casi todo (appsock `answer()` en
   `appsock.rs:347`), pero solo sobre un socket Unix local y atado a la ventana abierta.
2. **Sin identidad de actor en los cambios:** el journal de expense registra la fuente (`cli` o
   `mcp`), no quién. Brújula sí tiene `actor`.
3. **Sin instantánea ni eventos:** `subscribe` está "planned" en `docs/app-socket.md:73-77`.
4. **Sin capa `Backend`:** las vistas GPUI llaman a `Library` y a `store` directamente. Hay que
   meter un trait en el punto donde la app abre el libro, sin tocar cada vista (§4.2).
5. **Vigía, Nodo y Mi Huerta no tienen un registro de operaciones con nombre** como `ops::OPS`.
   Vigía tiene los verbos en la CLI (`add`, `edit`, `renew`, `delete`), que son la base natural.
6. **El Helper no tiene red ni procesos de larga vida.** Hoy todo es corrida → salida, con timeout
   de hasta 3600 s.
7. **Sin otherwise-ui en las apps:** el `ServerSwitcher` exige adoptar `Sidebar` primero, o dejar
   que la app incruste el componente en su barra propia.
8. **Licencias inexistentes en código:** VC-249 todavía no está en otherwise-link. Surco y Voltia
   tienen el gancho `features[]`.
9. **El modo Empresa de Brújula está en construcción** (rama `feat/empresa-mode`, `docs/EMPRESA.md`,
   con otherwise-ui), pero sigue siendo monousuario. Según `EMPRESA.md`, las firmas "llegará[n]
   con la versión multiusuario y el servidor". Tiene `ROLES` (`store.rs:53`), y quién hospeda el
   servidor sigue "pendiente de la reunión de socios" (`ARCHITECTURE.md:76-83`).
10. **Windows no tiene appsock;** irrelevante para el servidor (el Studio es macOS), pero sí para
    los clientes.

---

## 4. Diseño de la API

### 4.1 Transporte

- **HTTP/1.1 + JSON** detrás de `tailscale serve` (TLS con el certificado `*.ts.net` del tailnet).
  Es simple de depurar con `curl`, funciona en iOS, Android y Rust (`ureq`, que las apps ya usan)
  y no exige un runtime async en el Helper: un pool de hilos basta para pocos usuarios.
- **Eventos en vivo por SSE** (`GET …/events`, `text/event-stream`), no WebSocket:
  - es unidireccional, que es lo que hace falta (las escrituras van por POST);
  - reconecta con `Last-Event-ID` = revisión o cursor;
  - pasa por el proxy de `tailscale serve` sin configuración;
  - no necesita otra dependencia.

  WebSocket queda para presencia en vivo, si algún día hace falta.
- **MCP por streamable HTTP** en `/v1/mcp` (agregado, como `otherwise-hub`) y
  `/v1/apps/<app>/mcp`. El Helper reutiliza `otherwise_mcp_hub::child` contra los `<cli> mcp` del
  servidor. Así un agente en el portátil (el panel ⌘J) consulta los datos del Studio con las mismas
  herramientas. Las escrituras siguen la regla de hoy: `mcpWrites`, `--allow-writes`, y el libro
  debe permitir `agent_writes`.
- **Entre el Helper y `<app> serve`:** stdio con JSON por línea, el formato de appsock y del MCP
  actual. Una conexión por proceso hijo, y el Helper serializa las escrituras por libro.

### 4.2 El trait `Backend` en las apps (y por qué "Local" no es HTTP)

La propuesta del coordinador era que Local fuera el mismo servidor en localhost. Se evaluó así:

| | Local = HTTP en localhost | Local = `LocalBackend` en proceso (recomendado) |
|---|---|---|
| Caminos de código | 1 transporte | 1 API (trait), 2 transportes; las pruebas comparten casos |
| Dependencia del Helper | La app no abre un libro sin el Helper | Ninguna (como hoy) |
| Latencia y lecturas | Salto HTTP y serialización por cada consulta de vista | Lectura SQLite directa |
| Coste de migración | Reescribir todas las vistas a llamadas de API | Envolver la apertura y la escritura; las vistas siguen leyendo `Library` |
| Windows (sin appsock) | Necesita TCP local con token | Sin cambio |
| Seguridad local | Cualquier proceso del usuario (¡los runners de CI!) llega a localhost | Sin superficie nueva |

**Recomendación:** cada núcleo define un trait pequeño, por ejemplo para expense:

```rust
trait BookBackend {
    fn hello(&self) -> Hello;                                 // revisión, escribible, rol
    fn reader(&self) -> &Library;                             // lecturas (local: el libro; remoto: réplica RO)
    fn run(&mut self, op: &str, args: Value, ctx: OpCtx) -> Result<Value, OpError>;
    fn undo(&mut self, id: Option<&str>) -> Result<Value, OpError>;
    fn propose(&mut self, p: Proposal) -> Result<Value, OpError>;
    fn subscribe(&mut self) -> Receiver<Changed>;
}
```

`LocalBackend` es lo de hoy: `ops::run` sobre la `Library` abierta, y sigue sirviendo appsock.
`RemoteBackend` mantiene una **réplica de solo lectura** en
`device_dir()/remote/<server>/<app>/<book>/expense.db` y manda `run`/`undo`/`propose` por HTTP. Las
~96 operaciones y las ~53 herramientas de lectura **no se reescriben**: son el contrato.

El coste real de "salir de SQLite directo" se reduce a dos cosas:
1. las **escrituras** que la UI hace hoy fuera de `ops::OPS` (hay que inventariarlas: ediciones de
   ajustes, importaciones desde la vista) se convierten en operaciones;
2. la UI debe tolerar que `reader()` cambie bajo sus pies, por la recarga tras `changed`, que ya
   hace hoy con `changed_elsewhere()` (`library.rs:1187`).

### 4.3 Recursos HTTP (v1)

Prefijo `https://<máquina>.<tailnet>.ts.net/v1`.

| Método y ruta | Qué |
|---|---|
| `GET /hello` | `{server: "otherwise", version, protocols: {min, max}, host, me: {login, name, device}, apps: [{id, version, protocol, docs: […]}]}`. Sin datos de negocio. Lo usa "Añadir servidor". |
| `GET /apps/<app>/docs` | Libros, espacios de trabajo o mapas visibles para mí, con rol. |
| `GET /apps/<app>/docs/<doc>/hello` | El `hello` de appsock más `role`, `writable` (licencia + rol) y `revision`/`cursor`. |
| `GET /apps/<app>/docs/<doc>/snapshot` | **Réplica** (expense, Vigía, Nodo): un SQLite `VACUUM INTO`, con `ETag: <revision>`, `Content-Encoding: zstd` opcional y `If-None-Match` → 304. Documentos JSON (Mi Huerta…): el JSON con `revision`. |
| `POST /apps/<app>/docs/<doc>/ops` | `{op, args, base_revision?, idempotency_key, command?}` → `{ok, result, revision, change_id}` o un error appsock (`conflict`, `read_only`, `locked`, `forbidden`, `license`…). |
| `POST /apps/<app>/docs/<doc>/undo` | `{change_id?}`: deshace **mis** cambios (o cualquiera si soy `owner`). |
| `POST /apps/<app>/docs/<doc>/propose` | Igual que appsock `propose`: una tarjeta pendiente que alguien con rol `editor` acepta. |
| `GET /apps/<app>/docs/<doc>/events` | SSE: `changed {revision, tables, actor}`, `proposal`, `job {id, status}`, `bye {reason}` (actualización o apagado). |
| `GET/POST /apps/brujula/docs/<ws>/oplog?after=<cursor>` | Pull y push del op log (§5.2). |
| `POST /apps/expense/docs/<doc>/inbox` | Subida de recibos (sustituye a `phone.rs` para el móvil remoto), con las mismas invariantes: magic bytes, nombre generado y tamaño máximo. |
| `GET /audit?app=&doc=&since=` | Solo para `owner`. |
| `POST /mcp`, `POST /apps/<app>/mcp` | MCP streamable HTTP. |

**Cada operación lleva `idempotency_key`** (uuid del cliente). Si el cliente reintenta tras un
timeout, el servidor devuelve el mismo resultado sin escribir dos veces. Es el mismo patrón
compare-and-set e idempotente que usa Vcity en `customerSubscriptions` (plan de ventas, slice 1).

### 4.4 Modelo de operaciones por app

| App | Unidad (`doc`) | Escritura | Lectura remota | Notas |
|---|---|---|---|---|
| expense Business / me | Libro (carpeta) | `ops::OPS` por nombre (ya JSON-schema) | Réplica SQLite | `approve` y `close_month` vetados a `mcp` (ya); se añade el veto por rol |
| Brújula | Espacio (`.brujula`) | Op log: `Op{id, hlc, actor, kind, entity, field, value}` | Réplica local completa (es su modelo) | El servidor asigna `seq` y valida rol y módulo |
| Vigía | `vigia.db` | Ops nuevas desde la CLI: `add_item`, `edit_item`, `renew_item`, `delete_item`, `relate`… | Réplica SQLite | Los jobs reminders y checks corren **solo** en el servidor |
| Nodo | Mapa | `update_device`, `add_service`, `scan` | Réplica SQLite | El Studio es el mejor escáner: siempre encendido y dentro del tailnet |
| Mi Huerta | Jardín | Ops = propose_* que ya existen, aplicadas en el servidor | JSON con `revision` | `forecast` solo en el servidor |
| genealogy, Surco, Voltia, Helio, CACFU | Archivo | `PUT` del documento entero con `If-Match: <revision>` | `GET` del documento | "Abrir desde servidor" y "Guardar en servidor"; ningún multiusuario real |

### 4.5 Versionado

- **Ruta `/v1`:** cambia solo si se rompe el HTTP. Dentro de ella, `protocol` por app (appsock ya
  tiene `PROTOCOL = 1`). Como en `app-socket.md`, solo se añaden operaciones y campos nuevos; los
  campos desconocidos se ignoran.
- **Negociación:** `hello` devuelve `protocols: {min, max}`.
  - Un cliente más nuevo que el servidor (op desconocida) recibe `unknown_op` y la UI la oculta.
  - Un cliente **más viejo** que el esquema del servidor queda en **solo lectura**, con el aviso
    "Actualiza expense para editar en este servidor". Es la misma regla del plan de Vcity: "an
    older app facing a newer database opens read-only".
- **Réplica con un esquema que el cliente no conoce** (`BOOK_SCHEMA` mayor): no se abre. Se ofrece
  actualizar.
- **Ops con esquema:** `GET …/hello` incluye un hash de `OPS` (la operación `tools` planeada en
  appsock), para detectar desajustes sin bajar todo el catálogo.

### 4.6 Comportamiento sin conexión

- **expense, Vigía, Nodo (fases 1-4):** sin conexión, la réplica sigue abierta en **solo lectura**,
  con la franja "Sin conexión con Mac Studio · datos de las 14:05". Las ediciones se deshabilitan,
  no se encolan.

  Motivo: muchas operaciones de expense dependen del estado (`auto_reconcile`, `apply_rules`,
  `close_month`), y reproducirlas a ciegas horas después es peor que no dejar editar. La captura de
  recibos sí se encola: los archivos van al Inbox local pendiente y se suben al volver.
- **Fase posterior (opcional):** un outbox de ops "seguras" (las de forma `set_*` sobre un id), con
  `base_revision` por fila; el servidor las rechaza con `conflict` si la fila cambió.
- **Brújula:** offline de verdad desde el día uno. Escribe en su op log local (`synced = 0`) y al
  reconectar hace push de `unsynced()` y pull desde el cursor. Las operaciones que el servidor
  rechaza por rol vuelven como `rejected`, y el cliente las revierte con una tarjeta explicativa.
- **Mi Huerta y documentos:** en solo lectura offline, o "Guardar una copia local".

---

## 5. Concurrencia y consistencia

### 5.1 expense (libro)

- **Único escritor:** el proceso `expense serve --book <dir>` del servidor abre el libro con
  `Library::open` y toma `expense.lock`, lo que mantiene fuera a cualquier otro computador. El
  Helper serializa las peticiones `ops` por libro (una cola). Cada operación es una transacción y
  una entrada del journal, con `source = "remote"` y **`actor = <login>@<device>`** (campo nuevo
  del journal).
- **Si la ventana de expense está abierta en el propio Studio,** la ventana es el escritor y
  `serve` le reenvía las operaciones por appsock (como hace hoy la CLI). Es la regla de SPEC §11.2,
  sin un segundo mecanismo.
- **Conflictos:** `revision` sube con cualquier cambio, así que exigir `base_revision == revision`
  rechazaría casi todo con dos usuarios activos. Se propone:
  - las ops `set_*` sobre un id aplican LWW por campo, y el journal deja ver qué se pisó;
  - las ops estructurales (`link`, `merge_contacts`, `close_month`, `record_payment`) validan sus
    invariantes en el servidor (ya lo hace `ops::run`: `locked`, `exists`…);
  - `base_revision` es opcional, para operaciones que el usuario confirmó viendo un estado ("aplicar
    estas 12 reglas"); si no coincide, `conflict` con las filas.
- **Deshacer:** ⌘Z deshace *mis* cambios; `Undo` con un `change_id` ajeno solo para `owner`. Ya
  existe `already_undone` y `conflict` para un undo sobre filas cambiadas.
- **Los jobs** (mail, inbox, reminders) corren en el servidor con el lease de SPEC §11.3 y escriben
  a través del `serve` activo. Si nadie lo tiene abierto, toman el lock directamente, como hoy.
- **expense me y Business sobre el mismo data root:** sin cambio. El libro de empresa abierto desde
  me sigue en solo lectura (`me/src/edition.rs`).

### 5.2 Brújula (op log)

- Lo que ARCHITECTURE.md:71-72 ya define: un cursor de servidor por espacio, push de `unsynced()`,
  pull desde el cursor, `merge` y marcar `synced`. Los blobs se suben por hash SHA-256 (`PUT
  /blobs/<sha>`, idempotente).
- **El servidor es autoridad de orden y de permisos:**
  - asigna `seq`;
  - rechaza las operaciones que el rol o el módulo no permiten (`ARCHITECTURE.md:86-108`);
  - sella aprobaciones y firmas (Ley 527) con su reloj;
  - añade una cadena de hash sobre el log.

  La convergencia sigue siendo la de `State::apply` (LWW por (registro, campo), con orden
  `(hlc, actor, op id)`). Dos clientes con las mismas operaciones dan el mismo `digest()`, que se
  usa como prueba de convergencia en los tests.
- **Reloj:** `Clock::observe` ya acota sellos de más de un día en el futuro. El servidor rechaza
  además los sellos HLC que se adelanten más de N minutos a su reloj.
- **Códigos humanos** (R-007): solo los asigna el servidor; el cliente muestra "R-?" hasta el pull.

### 5.3 Vigía

Es SQLite mutable, sin op log. Se aplica el mismo patrón que expense: `vigia serve` como único
escritor, con operaciones con nombre extraídas de los subcomandos de la CLI (`crates/cli/src/
main.rs:299-544`) y movidas a `vigia-core` como un `ops.rs`. `reminders_sent` y los checks
RDAP/TLS solo los escribe el servidor, lo que también arregla los avisos duplicados entre equipos.

### 5.4 Nodo

El mapa SQLite con `monitor` corre en el servidor. Las escrituras son pocas (`update_device`,
`add_service`), con LWW por campo. El escaneo del Studio ve el tailnet completo. El P2P de archivos
(TCP 47821) queda fuera del servidor.

### 5.5 Mi Huerta y apps de documento

Compare-and-set del documento completo con `revision`, como ya hace `garden.json` ("refuse to
overwrite a newer revision"). En Mi Huerta las propose_* se aplican en el servidor, así que hay
escritura fina sin tocar el formato. En Surco, Voltia, Helio, CACFU y genealogy, el servidor
guarda y sirve archivos y el lock de otherwise-link pasa a ser un "check-out" en el servidor. Valor
bajo; última fase.

### 5.6 Reglas generales

- **Ningún cliente abre jamás los archivos del servidor por su cuenta** (ni por OneDrive ni por
  SMB). Si el data root del Studio estuviera en OneDrive, un cliente con esa carpeta sincronizada
  podría abrirla directamente. Por eso se recomienda que el data root del servidor esté en disco
  local y que OneDrive sea solo un destino de copia (§8.2).
- **Migración "Publicar en el servidor":** se copia el libro local al servidor con la copia
  verificada de `relocate` (`integrity_check`, `application_id`, contenido), por un `PUT`
  troceado. Después, la copia local queda marcada como "movida a Mac Studio" en solo lectura, para
  no tener dos verdades. Nunca se borra (regla del hub).

---

## 6. Seguridad

### 6.1 Red, ACLs e identidad

- **Solo el tailnet.** `tailscale serve --bg --https=443 http://127.0.0.1:47900`; nada de Funnel.
  El servidor **se niega a arrancar** si escucharía fuera de loopback o de un socket Unix.
- **ACL de Tailscale:** se propone una etiqueta para el Studio (`tag:otherwise-server`) y una
  regla que solo deje entrar a los usuarios del grupo `group:otherwise` al puerto 443 de ese
  equipo.

  Importante: **`tailscale serve` añade `Tailscale-User-Login` y `Tailscale-User-Name` solo si el
  origen es un nodo de usuario**, no uno etiquetado. Por eso los clientes (portátiles, móviles) no
  se etiquetan. Hay que comprobarlo en el tailnet real (pregunta abierta).
- **Identidad en dos capas:**
  1. las cabeceras de `tailscale serve`;
  2. **`whois` del LocalAPI** sobre la IP y puerto de origen, para obtener el nodo, el dispositivo
     y el usuario. Si falla o no coincide con las cabeceras, la respuesta es `401`.

  En el Studio, Tailscale es la app de macOS (`application.io.tailscale.ipn.macos`, sin `tailscale`
  en el `PATH`). El CLI está en `/Applications/Tailscale.app/Contents/MacOS/Tailscale`, y el
  acceso al LocalAPI desde otro proceso hay que verificarlo con esa variante (pregunta abierta). La
  alternativa es usar `tsnet` en el servidor (un nodo propio `otherwise-studio`), que da `WhoIs` en
  proceso, pero añade Go o un binario aparte.
- **Localhost no es confianza.** En el Studio corren **3 runners de GitHub Actions**
  (`~/Vcity/actions-runner{,-2,-3}`) **con el mismo usuario macOS** que corre OpenBot y que
  correría el Helper. Un workflow (por ejemplo, el código de un PR) podría llamar a
  `127.0.0.1:47900` y falsificar las cabeceras `Tailscale-User-*`. Mitigaciones, de mejor a peor:
  1. **Recomendada:** mover los runners a otro usuario macOS. Así los sockets 0700 y el keychain
     quedan fuera de su alcance. Es un cambio en el Studio que **decide el usuario**; este plan no
     lo hace.
  2. Que el backend escuche en un **socket Unix** en un directorio `0700` y que `tailscale serve`
     haga de proxy hacia él (soporte de `unix:` en serve, por verificar), con peer-uid. Con el mismo
     uid que los runners no basta, así que esto solo sirve combinado con la opción 1.
  3. Un **secreto compartido entre `tailscale serve` y el backend** no es posible (serve no añade
     cabeceras personalizadas). Por eso `whois` sobre el puerto de origen es lo que distingue una
     conexión real de serve: viene de tailscaled con la dirección del par, mientras que una conexión
     local directa no tiene par del tailnet.
- **Expuestos por app:** solo las apps instaladas en el servidor, y por libro solo los que el
  `owner` marcó como compartidos. Un libro no compartido no existe para los demás: es `404`, no
  `403`.

### 6.2 Roles

Hay roles genéricos por (miembro, app, documento) y Brújula añade los suyos encima:

| Rol | Puede |
|---|---|
| `owner` | Todo, incluidos miembros, compartir, auditoría, undo ajeno y ajustes del servidor |
| `editor` | Ops, undo propio y aceptar propuestas |
| `viewer` | Solo réplica y lecturas |
| `agent` | Lo que permitan `mcpWrites` y `agent_writes`, nunca `approve` ni `close_month` |

En Brújula, los `ROLES` de `store.rs:53` (admin, process_owner, sst_lead, internal_auditor,
external_auditor, management) se mapean dentro de `editor`/`viewer`, con `valid_until` para el
auditor externo.

La configuración (`members.json`: login de Tailscale → nombre → roles por app y documento) vive en
`device_dir()/server/` del servidor: **es de la máquina, no del data root**. La edita el `owner`
desde "Ajustes del servidor" (§7).

### 6.3 Auditoría

- `device_dir()/server/audit/<yyyy-mm>.jsonl`, solo anexado, una línea por petición que escribe y
  por inicio de sesión:

  ```json
  {"at": "…", "login": "…", "device": "macbookpro", "app": "expense", "doc": "…", "op": "set_receipt",
   "args_sha256": "…", "result": "ok", "revision": 1234, "change_id": "c12"}
  ```

- **Sin argumentos en claro:** el detalle está en el journal del libro, que es dato del usuario.
- Además, el journal de cada libro recibe `actor`, y Brújula ya tiene `actor` por operación más la
  cadena de hash del servidor.
- Rotación mensual; las lecturas no se registran, salvo la descarga de réplicas (una línea por
  `snapshot`).

### 6.4 Nada de secretos por el cable

- Las contraseñas IMAP (`expense-imap`) y de PDF de extractos (`expense-statement-pdf`) **solo
  viven en el keychain del servidor**. Los importadores corren en el servidor como jobs del Helper
  (`expense background run mail`) con el binario de la app, que es quien tiene el acceso al
  keychain (`cli/src/lib.rs:45`: la CLI sin ventana no recibe keychain; se usa el binario de la
  app, como ya hace el Helper).
- **Configurar una cuenta de correo en un libro remoto:** la vista de ajustes de expense muestra
  "Esta cuenta se configura en Mac Studio" y el estado (última lectura, error), nunca la
  contraseña. Para escribirla hay dos opciones (pregunta abierta):
  - (a) sentarse en el Studio o entrar por Compartir pantalla;
  - (b) un endpoint **de solo escritura** `PUT …/secrets/<name>`, solo para `owner`, que guarda en
    el keychain del servidor y nunca devuelve el valor.

  Se recomienda (a) en las fases 1-2.
- **Tokens:** no hay tokens propios del servidor (la identidad es Tailscale). Las licencias de
  Vcity no viajan (§10). Las réplicas sí son datos sensibles: se guardan en `device_dir()` del
  cliente, con permisos 0600, y se borran al "Quitar servidor".
- `phone.rs` (HTTP plano en la LAN con token) sigue igual en Local. En remoto lo sustituye el
  endpoint `inbox` por HTTPS e identidad.

---

## 7. UX del selector de servidor (tipo OpenBot)

**Dónde:** en `Sidebar::header` de otherwise-ui, en el lugar de la marca, como el selector de
espacios de OpenBot. Componente `otherwise_ui::ServerSwitcher`, solo vista: recibe
`Vec<ServerItem{id, name, tile: Accent, state, active}>` y callbacks. Las apps que todavía no
adoptan `Sidebar` (expense, Brújula…) lo incrustan arriba de su barra propia. Adoptar `Sidebar` es
el camino largo.

```
┌───────────────────────────┐
│ [MS] Mac Studio       ▾   │  ← tile de color con iniciales; punto de estado
│      expense · Empresa X  │
├───────────────────────────┤
│ SERVIDORES                │
│ [L]  Local            ✓   │
│ [MS] Mac Studio     ● ▸   │──┐  Silenciar servidor
│                           │  │  Notificaciones…
│ + Añadir un servidor      │  │  Ajustes del servidor…   (solo owner)
│ ⚙ Ajustes del servidor    │  │  Ocultar servidor
│ − Quitar servidor         │  └  Quitar servidor…
│ Diseño: Menú de servidor  │
└───────────────────────────┘
```

**Estados** (el punto del tile y una franja bajo la cabecera de la página):

| Estado | Señal | Comportamiento |
|---|---|---|
| Conectado | Punto verde | Normal |
| Conectando o sincronizando réplica | Punto girando | Muestra la réplica anterior si existe |
| Sin conexión | Punto gris y franja "Sin conexión · datos de 14:05" | Solo lectura (§4.6) |
| Solo lectura | Candado y franja con el motivo | Rol `viewer`, sin licencia, miembro por encima del tramo tras la gracia, cliente más viejo que el servidor, o libro de empresa en expense me |
| Incompatible | Punto ámbar | "Actualiza la app" o "El servidor tiene una versión más vieja" |
| Sin acceso | Punto rojo | `401`/`403`: "Pide acceso al dueño del servidor" |

**Añadir un servidor:**
1. La lista "En tu tailnet" lee los pares del tailscaled local (LocalAPI `status`) y sondea en
   paralelo `https://<peer>/v1/hello` con un timeout de 1,5 s. Solo muestra los que responden como
   `otherwise`, con sus apps.
2. Pestaña "URL" para escribir `https://studio.<tailnet>.ts.net` o una dirección on-prem.
3. Más adelante, "Vcity" con inicio de sesión Vcity (device flow ya existente en
   `crates/core/src/vcity.rs`).

Al elegir, se muestran los documentos visibles (`GET /apps/<app>/docs`) con el rol de cada uno.

**Por app, no global:** cada app recuerda su servidor y documento activos
(`device_dir()/servers.json`: lista de servidores compartida por todas las apps, más
`active[<app>]`). Que expense esté en el Studio no obliga a que lo esté Mi Huerta. El hub puede
mostrar una vista "Servidores" con todo.

**Notificaciones:** el outbox del servidor se drena en el servidor. Los clientes reciben
`job`/`notify` por SSE solo si no silenciaron el servidor ("Silenciar servidor"; las reglas finas
van en "Notificaciones…").

**Ajustes del servidor** (solo `owner`, por la API): miembros y roles, documentos compartidos,
límites (§8.4), auditoría y estado de los jobs.

---

## 8. Operación en el Mac Studio

Observado por SSH en modo solo lectura el 2026-11-24:

| Dato | Valor |
|---|---|
| Equipo | Mac Studio M2 Max, 12 núcleos, **32 GB**, macOS 26.2 |
| Disco de datos | 460 GiB, **84 % usado, 69 GiB libres** |
| Encendido | 7 días encendido; `sleep 0` (OpenBot lo impide); `autorestart 1`; `womp 1` |
| Tailscale | La app (`application.io.tailscale.ipn.macos`); **sin configuración de `serve`** |
| Corriendo | Ollama (`com.ollama.ollama`, `com.local.ollama-env`, más el LaunchDaemon `com.local.iogpu-wired-limit`, que reserva memoria GPU cableada); Docker (`com.docker.backend` ~800 MB); **3 runners de GitHub Actions** en `~/Vcity/actions-runner*`; OpenBot (~650 MB); OneDrive; varias sesiones `claude` |
| Memoria | 86 % libre en el momento de la muestra |
| Time Machine | **Sin destino configurado** |
| Apps Otherwise | **Ninguna instalada** |

### 8.1 launchd

- El servidor necesita **sesión gráfica de usuario**, por el keychain de `expense-imap` y las
  notificaciones. El Studio ya la tiene (OpenBot corre en ella). Por eso el modo servidor va en el
  mismo Otherwise Helper registrado como login item (`SMAppService`, `mac.rs`), con el interruptor
  "Modo servidor" en Ajustes → Otherwise Helper. **No** se usa un LaunchDaemon: no tendría keychain
  de usuario.
- `KeepAlive`: el login item de `SMAppService` lo reinicia si muere. Como respaldo, un comando
  `otherwise-helper doctor` que comprueba `serve`, el puerto y `tailscale serve status`.
- **Arranque tras un corte de luz:** `autorestart 1` ya está puesto. Hace falta inicio de sesión
  automático del usuario (o FileVault desbloqueado) para que haya sesión. Lo decide el usuario.
- `tailscale serve --bg` persiste su configuración en tailscaled. El Helper solo **comprueba** que
  existe y la muestra en Ajustes; nunca la cambia solo. El comando a copiar aparece en Ajustes, igual
  que el hub con `claude mcp add`.

### 8.2 Copias de seguridad

Con el servidor como único escritor, hay un solo lugar que respaldar y nadie más que lo respalde:
el Studio no tiene Time Machine.

- **Nocturna, por documento:** `VACUUM INTO` en SQLite (o una copia del JSON) hacia
  `~/.otherwise-backups/<app>/<doc>/<yyyy-mm-dd>.db.zst`, verificada con `integrity_check` y
  `application_id`, igual que `relocate`. Se conservan 14 diarias, 8 semanales y 12 mensuales.
  Es un job más del Helper: `backup`, `heavy`, `daily 03:30`.
- **Fuera de la máquina:** copiar el directorio de copias (no los vivos) a OneDrive, que ya corre en
  el Studio, y/o a un disco externo. El destino es una pregunta abierta. Un `.zst` cerrado en
  OneDrive es seguro; un `.db` vivo no.
- **Antes de cada migración de esquema y de cada actualización:** una copia previa, que ya es regla
  del plan de Vcity ("a backup taken before migrating").
- **Espacio:** con 69 GiB libres, las copias tienen un techo (por ejemplo, 10 GiB) y se avisa al
  `owner` al pasar el 90 % del disco.

### 8.3 Actualizaciones de las apps en el servidor

- Las apps se instalan en el Studio con Otherwise Apps, por la vía verificada de siempre (digest
  SHA-256 de GitHub y, más adelante, la firma ed25519 de VC-254). El Helper **no** instala nada (es
  regla).
- **Al detectar un bundle nuevo** (`exe_identity`: inode y mtime, que ya existe), el Helper:
  1. deja de aceptar escrituras para esa app (`503` con `Retry-After`);
  2. manda `bye {reason: "update"}` por SSE;
  3. espera a que el `serve` vacíe su cola y lo para;
  4. arranca el nuevo, que migra el esquema con copia previa.

  Los clientes reconectan solos.
- **Desfase de versiones:** el servidor más nuevo que el cliente deja al cliente en solo lectura,
  con el aviso de actualizar (§4.5). Las actualizaciones gratuitas para todos (plan de Vcity) lo
  hacen raro.
- **"Actualizar todo en el servidor"** es una acción de Ajustes del servidor que abre Otherwise Apps
  en el Studio. Las actualizaciones automáticas quedan para cuando exista el updater de VC-254.

### 8.4 Límites de recursos

El Studio comparte 32 GB con Ollama (memoria GPU cableada), Docker y 3 runners de CI. Valores
propuestos:

| Límite | Valor |
|---|---|
| Por proceso `serve` | `memoryMb` 300-500 (lo declara la app en su `otherwise-background.json`, nueva sección `serve`), `nice 10`, su propio grupo de procesos (el `runner.rs` de hoy) |
| Presupuesto total del servidor | 2 GB; por encima, se paran primero los `serve` inactivos |
| Inactividad | Un `serve` sin peticiones ni suscriptores SSE durante 10 min se para; arranca en frío bajo demanda (expense abre un libro en menos de 1 s) |
| Jobs `heavy` | Se mantiene que nunca corren dos a la vez y que se saltan con presión de memoria (`sched.rs`). Se añade una pregunta abierta: no correrlos mientras haya un job de CI activo, detectado por `Runner.Worker` vivo, que es fácil con `sys.rs` |
| Ollama | El servidor no usa Ollama. El extractor de recibos de expense que llame a un LLM local queda como pregunta abierta, porque compite con lo que ya corre |

---

## 9. Clientes móviles

- **Transporte:** la app Tailscale del móvil, más el mismo contrato `/v1`. No hace falta nada
  especial en el servidor. Los móviles del tailnet (iPhone, iPad, Android) ya aparecen en `tailscale
  status`.
- **Primer alcance, de solo lectura y JSON:** los móviles no deben bajar una réplica SQLite
  completa. Se añaden endpoints de **consulta** que reutilizan las herramientas de lectura del MCP
  tal cual (`POST /apps/expense/docs/<doc>/query {tool: "payables", args}` →
  `ai_tools::TOOLS`). Así el móvil tiene ~53 consultas de expense y 9 de Vigía sin código nuevo en
  el núcleo.
- **Escritura móvil:**
  - captura de recibos (`inbox`, sustituye a la página de `phone.rs` cuando hay servidor);
  - `mark_paid` y `renew_item`: unas pocas ops de un toque.
- **Brújula móvil** (ROADMAP: "app móvil"): op log y réplica local, más adelante.
- **Tecnología** (pregunta abierta): Mi Huerta ya separa `huerta` "portable para una app móvil";
  la opción natural es un núcleo Rust con UniFFI. Para clientes de solo consulta, una app nativa
  delgada sobre HTTP es más barata.
- **Notificaciones push:** fuera del tailnet no hay forma de despertar el móvil sin un servicio
  push (APNs/FCM), que implica Vcity cloud. Mientras tanto, el móvil consulta al abrirse. Pregunta
  abierta.

---

## 10. Encaje con la sincronización de Vcity y las licencias

### 10.1 Mismo contrato

El plan de ventas (`vcity/docs/plans/2026-11-23-software-sales-vcity.md`) deja la sincronización
"para después" y dice que "the data root is the natural unit to sync". Brújula plantea "Vcity as
the platform" como opción 1 (`ARCHITECTURE.md:76-83`). Se propone:

- Escribir `/v1` como **SPEC §12 de otherwise-link**: `hello`, `docs`, `snapshot`, `ops`, `undo`,
  `propose`, `events` (SSE), `oplog?after=` y `blobs/<sha>`, con errores con nombre.
- **El servidor Otherwise y Vcity cloud implementan el mismo contrato.** Solo cambian:
  - la **autenticación**: Tailscale frente a la sesión de Vcity (device flow, token en el keychain,
    `crates/core/src/vcity.rs`);
  - el **almacenamiento**: el Studio guarda los SQLite de las apps tal cual; Vcity guardaría el op
    log de Brújula en SurrealDB, como recurso del org.
- **El cliente no cambia:** `RemoteBackend` recibe un `ServerKind::{Tailnet, Vcity, OnPrem}` que
  solo elige cómo autenticar.
- **On-prem para clientes** (Brújula en una empresa, ROADMAP): es *exactamente* el servidor
  Otherwise en un Mac mini o en Linux (el Helper ya compila en Linux). La identidad puede venir del
  Tailscale del cliente, o de una cuenta Vcity si el cliente no usa Tailscale (fase posterior).
- **Qué no se reutiliza de Vcity:** no se pretende que el Studio hable Yjs ni Surreal. El nivel
  común es el contrato HTTP, no el motor.

### 10.2 Licencias: por tamaño de equipo (revisado 2026-11-24)

**Cambio respecto al plan de Vcity del 2026-11-23.** Allí la licencia era por persona y con 3
dispositivos. El usuario la cambió, tras revisar este plan, por una **licencia por tamaño de
equipo**, más telemetría mínima para comprobar que se respeta. El plan de Vcity y la estrategia de
la suite se corrigen en PRs aparte. Se mantiene todo lo demás:
- prueba gratis;
- 7 días de gracia tras un cobro fallido;
- 14 días offline;
- actualizaciones gratis;
- licencias regaladas;
- "sin licencia = solo lectura, exportar siempre funciona".

**Modelo:**

- **Dueño:** la licencia es del **equipo u org**, es decir, del `owner` que la compra en la tienda
  Vcity de la marca.
- **Límite:** fija un **máximo de miembros activos**.
- **Quién edita:** cualquier miembro del equipo se conecta a los servidores de ese equipo y edita
  dentro del tramo. El hermano, por ejemplo, es un miembro más. No hay comprobantes por persona ni
  tokens de cliente que viajen.
- **Tramos de ejemplo**, por decidir (precios y nombres son del plan de Vcity):

  | Tramo | Miembros activos | Pensado para |
  |---|---|---|
  | Individual | 1 | Una persona, sus equipos |
  | Pequeño | hasta 3 | Socios, familia (el caso usuario + hermano) |
  | Equipo | hasta 10 | Pyme |
  | Empresa | hasta 25 | Empresa mediana (Brújula modo Empresa) |
  | A medida | más de 25, u on-prem | Contrato aparte |

- **Dispositivos:** no hay límite duro por miembro. Se cuentan solo como señal en la telemetría.
  El límite de 3 dispositivos desaparece como regla.

**Conteo de puestos, en el servidor:**

- **Qué cuenta:** las **identidades distintas activas en una ventana móvil de 30 días**. Hoy son
  logins de Tailscale (`whois`, §6.1); con Vcity cloud serán cuentas Vcity.
- **Qué es "activo":** al menos una petición autenticada a un documento de una app de pago. El
  `hello` solo no cuenta.
- **Liberar un puesto:** el `owner` quita a un miembro en "Ajustes del servidor" y el puesto queda
  libre **en el acto**, sin esperar los 30 días. Queda registrado en la auditoría.
- **Licencia en el servidor:** el servidor tiene **el** token de licencia del equipo. Es estado del
  dispositivo, en el keychain del Studio, y lo renueva el Helper con la sesión Vcity del `owner`.
  Ese token trae `licence_id`, `tier`, `maxMembers`, `apps[]`, `features[]` y `validUntil`.
- **Sin servidor, en Local:** cada miembro activa su equipo con su cuenta Vcity dentro de la
  licencia del equipo (VC-251), y Vcity cuenta las cuentas distintas con tokens vivos. Es el mismo
  límite contado del otro lado.

**Telemetría mínima y que preserva la privacidad:**

- **Cuándo:** una vez al día, **dentro de la misma llamada de renovación del token** que el Helper
  ya hace. No hay conexión nueva ni SDK de analítica.
- **Qué se envía, exactamente:**

  ```json
  {
    "schema": 1,
    "licenceId": "lic_8f2c…",
    "serverId": "srv_3b9a…",
    "app": "expense",
    "appVersion": "0.9.3",
    "window": "30d",
    "activeMembers": 3,
    "memberHashes": ["9f1c2a7d0b4e6a11", "…"],
    "devices": 5,
    "activeDays": 22
  }
  ```

  Qué significa cada campo:
  - `serverId`: un uuid aleatorio de la instalación del servidor; no es el nombre del equipo.
  - `memberHashes`: el HMAC-SHA256 de cada identidad con una **sal secreta del servidor**, que nunca
    sale de él, truncado a 64 bits. Vcity puede deduplicar entre informes del mismo servidor, pero
    no puede revertir un hash ni cruzarlo con correos ni con otros servidores.
  - `devices`: los nodos de Tailscale distintos en la ventana.
  - `activeDays`: cuántos de los últimos 30 días hubo alguna escritura.

  Es una línea por app de pago y por servidor. Si un equipo tiene varios servidores, Vcity suma los
  miembros, lo que da una cota superior y favorece al cliente ante la duda.
- **Qué nunca se envía:** nombres, correos, logins, IPs del tailnet, nombres de libros o espacios,
  contenidos, montos, contrapartes, operaciones ni frecuencia por persona.
- **Divulgación:**
  - en la app: "Ajustes del servidor → Qué enviamos a Vcity", que muestra **el último JSON
    enviado, tal cual**;
  - en el texto de la licencia y en los términos de la tienda.
- **¿Obligatoria u opcional?** Se recomienda que sea **obligatoria en los tramos de pago**, porque
  es parte de la comprobación de licencia, con divulgación clara. Quien no la acepte tiene el tramo
  "A medida" (por ejemplo, licencia offline con conteo contractual). Las apps **gratis no tienen
  telemetría ni licencia**: expense me, Mi Huerta y las de código abierto (Nodo, genealogy). Su
  modo servidor, si existe, no informa nada.

**Aplicación suave:**

1. **Tramo + 1:** el `owner` ve una franja: "4 miembros activos este mes; tu plan Pequeño incluye
   3". Los miembros no ven nada.
2. **Gracia de 14 días** desde el primer día por encima del tramo, para subir de tramo o liberar un
   puesto.
3. **Después,** los miembros **por encima del tramo** pasan a **solo lectura** en ese servidor. Se
   eligen por orden de llegada: los más recientes primero. Nunca pasa a solo lectura el `owner`, y
   nunca se pierden, cifran ni ocultan datos. Exportar sigue funcionando.
4. **Servidor offline:** sigue aplicando el último tramo conocido durante la **ventana de 14 días
   ya decidida**. Pasado ese plazo sin contacto con Vcity, el servidor queda en solo lectura para
   todos hasta que renueve.

**Otros puntos:**

- **Apps de código abierto:** "la licencia cubre los servicios". El servidor Otherwise podría ser un
  servicio de pago para apps libres, o gratis para ellas. Es una decisión comercial pendiente; si
  fuera de pago, usaría el mismo tramo y la misma telemetría.
- **Brújula:** `modules.licensed_until` pasa a ser controlado por el servidor, que rechaza las
  operaciones de módulos no licenciados. El tramo por miembros reemplaza la idea de "por
  organización + módulos + usuarios" de `ROADMAP.md:61-62`; los módulos siguen siendo
  `features[]`.
- **Tarjetas de Vcity afectadas:** VC-246 (producto: `maxMembers` y tramo en lugar de
  `maxDevices`), VC-248 (entitlements, devices, licence API y tokens: el token pasa a ser de
  equipo, más el endpoint de telemetría), VC-249 (módulo de licencia en otherwise-link: conteo y
  telemetría en el servidor) y **VC-251** (licencias de equipo: deja de estar abierto, porque es el
  modelo base). El detalle está en el PR del plan de Vcity; no se editaron tarjetas del tablero.

---

## 11. Riesgos y preguntas abiertas

### Riesgos

| Riesgo | Mitigación |
|---|---|
| Los **runners de CI con el mismo usuario** llegan a loopback y podrían falsificar identidad o leer réplicas y el keychain | Runners en otro usuario macOS; `whois` obligatorio; socket 0700 (§6.1) |
| **Disco al 84 % y sin Time Machine:** el servidor concentra los datos en un solo disco | Copias con techo, copia fuera de la máquina y alarma de disco (§8.2) |
| **Un único punto de fallo:** si el Studio se apaga, nadie edita | Réplica en solo lectura en los clientes; el **Local** sigue existiendo; exportar un libro del servidor a Local ("Llevar a este Mac") con la copia verificada |
| **Desfase de versiones** entre 10 apps, sus servidores y otherwise-link (de 0.2.0 a 0.5.3) | Contrato aditivo, solo lectura para el más viejo, y adopción app por app |
| **Expectativa de tiempo real:** dos personas editando el mismo recibo | LWW por campo y journal con actor visible; edición en vivo fuera de alcance |
| **Coste de la capa `Backend`** en apps grandes (expense `app.rs` y las vistas) | Réplica en solo lectura: las lecturas no cambian; se inventarían las escrituras fuera de `ops::OPS` antes de la fase 2 |
| La variante de Tailscale del Studio no permite el LocalAPI desde el Helper | Verificar en la fase 1; alternativa `tsnet` |
| **Privacidad entre personas:** el libro personal (expense me) del usuario en un servidor compartido | Documentos no compartidos son invisibles (`404`); compartir siempre es explícito |
| **Trabajo duplicado con Vcity sync** | Un solo contrato, escrito como SPEC de otherwise-link y revisado con el plan de Vcity |

### Preguntas abiertas (para el usuario)

La anterior pregunta 2 (cómo edita el hermano) queda **resuelta** con la licencia por tamaño de
equipo: él es un miembro del equipo y ocupa un puesto del tramo (§10.2).

1. **Runners de CI en otro usuario macOS** del Studio: ¿sí? Sin eso, el servidor no puede distinguir
   con seguridad a un workflow de CI de un cliente legítimo en loopback. Además, ahora el keychain
   del Studio guarda el token de licencia del equipo.
2. **Dónde vive el data root del Studio:** ¿disco local con copias cifradas a OneDrive o a un disco
   externo, o seguir en OneDrive? Se recomienda disco local.
3. **Configuración de secretos remotos:** ¿solo en el Studio (Compartir pantalla), o un endpoint de
   solo escritura para `owner`?
4. **Sin conexión en expense:** ¿basta con solo lectura más captura de recibos encolada, o se
   necesita editar offline desde el principio? Editar offline cambia mucho el coste.
5. **Tramos y telemetría:**
   - ¿Los tramos 1 / 3 / 10 / 25 / a medida, y la gracia de 14 días, valen como punto de partida?
   - ¿El tramo es por app o por la suite completa?
   - ¿La telemetría es obligatoria en los tramos de pago, como se recomienda?

Otras, de menor prioridad:
- ¿`tailscale serve` hacia un socket Unix funciona en la variante instalada?
- ¿Las cabeceras de identidad funcionan con la política actual de etiquetas del tailnet?
- ¿El modo Empresa de Brújula se sirve desde el servidor Otherwise, o espera la decisión de los
  socios sobre Vcity como plataforma? El contrato es el mismo en ambos casos.
- ¿Vigía es de pago o libre? `apps::KNOWN` dice `open_source: false`, y el repo es MIT/Apache.
  Decide si su modo servidor lleva licencia y telemetría.
- ¿El servidor es un servicio de pago para las apps libres?
- ¿Inicio de sesión automático en el Studio tras un corte?
- ¿El extractor LLM de expense corre en el Studio (con Ollama)?
- ¿Las apps móviles serán nativas o con núcleo Rust (UniFFI)?

---

## 12. Qué apps tendría más sentido hacer ahora

Se ordenan por el **valor del uso multiusuario y siempre encendido** frente al **coste**, medido
contra el código. Coincide con el orden que propuso el coordinador, con un matiz sobre Nodo.

| # | App | Valor multiusuario | Coste (lo que ya existe) | Veredicto |
|---|---|---|---|---|
| 1 | **expense Business** | Alto: el usuario y el hermano ya comparten libros por OneDrive, con el lock "abierto en otro computador"; los jobs de correo y extractos ganan por correr siempre en un solo lugar | **El más bajo:** appsock (único escritor, `revision`, `undo`, `propose`), 96 `ops::OPS` con esquema, CLI sin ventana "para servidores", jobs listos | **Ahora** (fases 1-2) |
| 2 | **Brújula** | Muy alto: el modo Empresa (`feat/empresa-mode`) es multiusuario por naturaleza (personas que reportan, aprueban y firman); ROADMAP lo pide | Medio: op log con HLC y `unsynced()`/`merge()` listo para sync. Faltan el filtro por rol, la cadena de hash y los sellos del servidor. Depende de la decisión de los socios | **Después de expense** (fase 3), cuando el modo Empresa entre a main |
| 3 | **Vigía** | Medio: vencimientos y recordatorios compartidos en familia (SOAT, seguros, documentos); los jobs `reminders` y `checks` en un solo lugar evitan avisos duplicados | Bajo a medio: CLI completa con `--json`, pero SQLite mutable sin op log; hay que extraer un `ops.rs` de los verbos de la CLI | **Tercera** (fase 4) |
| 4 | Nodo | Bajo como multiusuario, **alto como "siempre encendido"**: el Studio es el mejor monitor del tailnet | Bajo: CLI, job `monitor` y MCP con escrituras | Junto a Vigía si sale barato; es libre, así que sin licencia |
| 5 | Mi Huerta | Bajo a medio (un jardín de familia) | Bajo: documento con `revision` y propose_* | Más adelante; gratis |
| 6 | Surco, Voltia, Helio, CACFU, genealogy | Bajo: son cálculos o archivos de una persona | Medio, porque no tienen `serve` y algunas ni MCP | Solo "abrir y guardar en el servidor", si alguien lo pide |
| — | **expense me** | No aplica: es la edición gratis y **personal** | — | **Se queda local**, sin modo servidor ni telemetría. Si el usuario quiere sus libros personales en el Studio, eso es expense Business (o un libro no compartido en el servidor) |

---

## 13. Plan por fases (cortes pequeños)

Cada fase deja algo usable y no rompe Local. Ninguna fase cambia la configuración del Studio sin que
el usuario lo pida.

### Fase 0: contrato (solo documentos y tipos)

- **Qué:** SPEC §12 en `~/otherwise-link/docs/SPEC.md`; crate `crates/remote` con los tipos serde
  (`Hello`, `DocInfo`, `OpRequest`, `OpResult`, `ApiError`, `Event`) y un cliente `ureq`+SSE contra
  un servidor de prueba; `servers.json` en `device_dir()`.
- **Archivos:** `otherwise-link/crates/remote/{Cargo.toml, src/lib.rs, src/types.rs,
  src/client.rs, src/sse.rs, src/registry.rs}`, `tests/contract.rs` (vectores JSON fijos) y
  `tests/client.rs` (servidor HTTP local de prueba; nunca red real, como los tests de Vcity del hub).
- **Prueba:** `cargo test -p otherwise-remote`. Los vectores JSON quedan como "golden" que también
  usará Vcity.

### Fase 1: piloto de expense Business, remoto en solo lectura

- **expense** (`~/expense-rs`):
  - `crates/cli`: subcomando `expense serve --book <dir> --stdio`. Reutiliza `appsock::answer`
    (`hello` y lecturas) y añade:
    - `snapshot`: `VACUUM INTO` a un temporal y devuelve la ruta;
    - `subscribe`: el planeado en `docs/app-socket.md`, emite `changed` cuando sube
      `meta.revision`.

    En esta fase, nada de `run`.
  - `core/src/backend.rs`: el trait `BookBackend` y `LocalBackend` (envuelve lo de hoy).
  - `app`: `RemoteBackend` con `otherwise-remote`. La réplica en `device_dir()/remote/...` se abre
    con `Library::open_read_only` y las ediciones se deshabilitan con la franja "Solo lectura ·
    Mac Studio".
  - Selector mínimo en la barra lateral: el componente de otherwise-ui si llega a tiempo; si no, uno
    provisional en `app/src/servers.rs` que se reemplaza en la fase 2.
- **otherwise-apps** (`~/otherwise-apps`):
  - `crates/server` (lib, sin GPUI): HTTP en 127.0.0.1, identidad (cabeceras + `whois`),
    `members.json` (solo el `owner`), router `/v1/hello`, `/v1/apps/expense/docs[/…/hello|snapshot|
    events]`, supervisor `serve` con parada por inactividad y presupuesto de memoria, y auditoría de
    `snapshot`.
  - `crates/helper/src/lib.rs`: arranca el servidor si "Modo servidor" está activo.
  - `crates/app/src/helper.rs`: el interruptor y el comando `tailscale serve` para copiar.
  - i18n en todos los catálogos.
- **otherwise-link:** `background.json` con una sección opcional `serve` (exe, args, memoryMb) en el
  manifiesto de la app. SPEC §11.1, aditivo.
- **Pruebas:**
  - unitarias de `crates/server`, con un binario falso de app como hacen los tests de
    `crates/helper/tests/engine.rs`: identidad rechazada sin `whois`, `404` para documentos no
    compartidos, parada por inactividad, techo de memoria y respuesta `304` con `If-None-Match`;
  - en expense: `serve` sobre un libro de prueba temporal, con la réplica en solo lectura, un
    `changed` tras una escritura de la CLI y la integridad de la instantánea;
  - **manual en el Studio**, solo cuando el usuario lo apruebe: instalar expense y Otherwise Apps,
    activar el modo servidor, `tailscale serve`, y desde el MacBook elegir "Mac Studio", ver el
    libro, escribir en el Studio con la CLI y ver que el MacBook se refresca; desconectar Tailscale
    y ver "Sin conexión".
- **Hecho cuando:** el usuario consulta desde el portátil el libro que vive en el Studio, y Local
  sigue idéntico.

### Fase 2: escrituras en expense, roles y auditoría

- **expense:**
  - `serve` acepta `run`, `undo` y `propose` (appsock completo, más `idempotency_key`);
  - el journal gana `actor`;
  - `RemoteBackend::run` y la UI habilitan las ediciones;
  - ⌘Z de *mis* cambios;
  - inventario y migración a `ops::OPS` de las escrituras de la UI que hoy no pasan por ahí;
  - en Ajustes de cuentas de correo, "Se configura en Mac Studio".
- **Servidor:**
  - roles `owner`/`editor`/`viewer`/`agent` y `members.json` editable desde "Ajustes del servidor";
  - auditoría de escrituras;
  - cola de escritura por documento;
  - jobs mail, inbox y reminders corriendo en el servidor (lease §11.3);
  - endpoint `inbox` (recibos);
  - **MCP streamable HTTP** `/v1/mcp` (proxy de `expense mcp` del servidor).
- **otherwise-ui:** `components/servers.rs` (`ServerSwitcher`) con estados y menú, y su ejemplo en
  `examples/gallery.rs`. expense lo usa.
- **Licencia:** si VC-249 ya existe, el servidor exige la licencia propia para escribir y los
  conteo de miembros más la telemetría de §10.2. Si no, el conteo y la franja de tramo funcionan
  igual en local y la telemetría queda apagada.
- **Pruebas:**
  - dos clientes de prueba escribiendo en paralelo, con orden serializado, la misma clave idempotente
    escrita una sola vez, y `conflict` con `base_revision` vieja;
  - `viewer` recibe `403` en `ops`;
  - el agente no puede `close_month`;
  - auditoría con hash de argumentos y sin argumentos en claro;
  - actualización del bundle durante una escritura (`bye` y reconexión).
- **Hecho cuando:** el usuario y su hermano trabajan sobre el mismo libro en el Studio sin OneDrive.
  Para eso hace falta antes la "Publicar en el servidor" (§5.6) con la copia verificada de
  `relocate`.

### Fase 3: Brújula multiusuario (modo Empresa)

- **brujula** (`~/brujula-rs`):
  - `brujula serve --workspace` con `oplog` pull/push, blobs por hash y códigos del servidor;
  - el cliente sincroniza `unsynced()` y `merge()` (la costura ya existe), y revierte las
    operaciones rechazadas;
  - se adopta `otherwise-ui` `Sidebar` más `ServerSwitcher`.
- **Servidor:** validación por rol y módulo de Brújula, cadena de hash y sello de aprobaciones.
- **Pruebas:**
  - convergencia: N clientes con operaciones barajadas acaban con el mismo `State::digest()`;
  - un auditor externo con `valid_until` vencido recibe rechazo;
  - offline → online con conflicto LWW;
  - cadena de hash verificable.
- **Requisito previo:** la decisión de los socios sobre "Vcity como plataforma" frente a servidor
  propio. El contrato es el mismo, así que esta fase no se pierde si gana Vcity.

### Fase 4: Vigía, Nodo, Mi Huerta (expense me se queda local)

- **Vigía:** `vigia-core/src/ops.rs` (de los verbos de la CLI) y `vigia serve`; jobs solo en el
  servidor.
- **Nodo:** `nodo serve`; `monitor` en el Studio.
- **Mi Huerta:** documento con `revision`, y propose_* aplicadas en el servidor.
- **expense me:** **sin modo servidor** (§12): es la edición gratis y personal, y se queda local.
- **Pruebas:** por app, `serve` sobre datos de prueba, más los mismos tests de contrato de la
  fase 0. El hub muestra la vista "Servidores".

### Fase 5: móvil

- Endpoints `query` (herramientas de lectura MCP como JSON) e `inbox`.
- Cliente móvil de solo consulta (expense: por pagar, cierre de mes; Vigía: vencimientos), más
  captura de recibos y `mark_paid`/`renew_item`.
- **Pruebas:** contrato con los vectores golden, y una prueba manual por Tailscale en iPhone y
  Android.

### Fase 6: apps de documento (opcional)

Surco, Voltia, Helio, CACFU y genealogy: "Abrir desde servidor" y "Guardar en servidor" con
`If-Match`, y el lock pasa a ser un check-out en el servidor. Se hace solo si hay demanda real.

### Fase 7: Vcity cloud y on-prem

- Vcity implementa SPEC §12 con la sesión Vcity.
- `ServerKind::Vcity` en `otherwise-remote`.
- Empaquetado on-prem del servidor: Linux con un `systemd` unit; el Helper ya es multiplataforma.

---

## 14. Decisiones (2026-11-24) e implementación

### 14.1 Decisiones del usuario

- **Alcance:** expense Business es la app multicliente y va primero. El Mac Studio hace todo el
  trabajo y queda siempre encendido: el libro, los importadores de correo y extractos, los
  recordatorios y las herramientas de IA. El MacBook Pro es un cliente del Studio.
- **expense me** se queda en un solo computador. Más adelante solo expondrá sus datos a un
  cliente móvil; **no** se construye multiusuario para expense me. El código compartido en
  expense-core queda reutilizable (el trait `BookBackend` no depende de la edición).
- **Licencias:** tramos por tamaño de equipo con conteo de puestos en el servidor (§10.2). En
  este piloto el conteo es **solo local y se registra en el log**; **no** se envía telemetría a
  Vcity (el endpoint de licencias de Vcity no existe todavía) y nadie pasa a solo lectura por
  puestos.

### 14.2 Valores por defecto (sin respuesta del usuario; registrados como decisiones)

- **Data root del Studio en disco local** (`~/.otherwise`, no OneDrive en vivo). Copias
  nocturnas a una carpeta aparte (`~/.otherwise-backups/<app>/<doc>/<yyyy-mm-dd>.db`, 14
  diarias, 8 semanales, 12 mensuales), hechas con la instantánea de la propia app.
- **Secretos (IMAP, contraseñas de extractos) solo en el keychain del Studio.** Los pone el
  usuario en el Studio (opción (a) de §6.4). No hay endpoint de secretos.
- **Sin conexión, el cliente queda en solo lectura** con la última instantánea (§4.6). No hay
  cola de escrituras offline.
- **Pregunta 1 (runners de CI en otro usuario):** sigue siendo decisión del usuario. Mientras
  tanto el código no confía en loopback (14.4).

### 14.3 Qué se construyó

| Pieza | Dónde | Qué |
|---|---|---|
| Contrato `/v1` | otherwise-link 0.6.0, `docs/SPEC.md` §12 | hello, docs, doc hello, snapshot (ETag/304), ops con `idempotencyKey` y `baseRevision`, undo, eventos SSE, roles, errores, puestos, auditoría, sección `serve` del manifiesto |
| Cliente | otherwise-link 0.6.0, crate `otherwise-remote` | tipos con vectores golden, cliente `ureq` + SSE, `servers.json` en el device dir, descubrimiento del tailnet con el CLI local de Tailscale y sondeo de `/v1/hello` |
| Vista | otherwise-ui 0.1.2, `ServerSwitcher` | Servidores (Local / Mac Studio) con check, submenú por servidor (Usar, Ajustes, Quitar), Añadir un servidor, estado como punto del tile; en `Sidebar::header` |
| `expense serve` | expense-rs, `cli/src/serve.rs` | el libro sin ventana: lock, el socket de la app (CLI, MCP y jobs del Studio escriben a través de él), revisión de correo con el keychain de la app, protocolo appsock por stdin/stdout con `seq`, `actor`, `base_revision`, `undo` propio o `any`, eventos `changed` |
| Núcleo | expense-core | `backend.rs` (`BookBackend`, `LocalBackend`), `appsock` `snapshot` y `actor`, `Change.actor`, `store::vacuum_into`, `background::mail_on` |
| App | expense-rs, `app/src/remote.rs` | `ServerSwitcher` en la barra lateral, Añadir un servidor (pares del tailnet o dirección), elegir libro, réplica de solo lectura con SSE y latido, franja de estado; `app_op` (panel del recibo, tarjetas, planes, simulador, limpiar comercios) y ⌘Z van al servidor; Local igual que antes |
| Servidor | otherwise-apps, crate `otherwise-server` + `otherwise-helper server …` | HTTP en 127.0.0.1:47900, identidad Tailscale, `server.json` (libros compartidos, miembros, roles por libro), supervisor de `expense serve`, auditoría, idempotencia persistente, puestos, copias nocturnas; `server run` corre además los jobs del Helper (correo, recordatorios, inbox) |

Cambios respecto al plan:

- El protocolo entre el Helper y `<app> serve` es stdio; la ventana de expense en el Studio,
  si está abierta, sigue siendo el escritor (serve le reenvía).
- El correo del libro servido lo revisa el propio `expense serve` (como lo haría la ventana);
  el job `mail` del Helper ve la app abierta y no duplica. Recordatorios e inbox siguen como
  jobs del Helper.
- La ventana de "Ajustes del servidor" (miembros, roles, compartir) es por ahora la línea de
  comandos del Studio (`otherwise-helper server init|share|member|tier|status|doctor`); en la
  app el menú muestra la dirección y remite a ella.
- Fase 2 en la app: escriben las acciones que ya pasan por operaciones (`app_op`); el resto de
  las ediciones de la UI (inventario de §4.2) sigue en solo lectura en remoto. Los archivos de
  los recibos (PDF) no se descargan todavía: la réplica trae el índice, no los documentos.
- MCP por HTTP (`/v1/mcp`), `inbox` remoto, "Publicar en el servidor" y notificaciones a los
  clientes por SSE quedan para después.

### 14.4 Seguridad implementada

- El servidor escucha solo en `127.0.0.1` y nunca cambia la configuración de Tailscale; imprime
  el comando `tailscale serve --bg --https=443 http://127.0.0.1:47900`.
- **Loopback no es confianza.** Cada conexión se verifica una vez: el otro extremo del socket
  (`lsof`) debe ser un proceso de la app de Tailscale con firma de Apple App Store para sus
  bundle ids, o Developer ID del equipo de Tailscale (`codesign --verify -R …`). Un runner de
  CI que llame a `127.0.0.1:47900` recibe `401` aunque escriba las cabeceras.
- Cada petición: `X-Forwarded-For` (el último valor, el que añade serve) debe ser del tailnet,
  `tailscale whois` debe conocerlo y coincidir con `Tailscale-User-Login`; Funnel se rechaza.
- Roles por libro; un libro no compartido es `404`; el agente escribe como `mcp` (las
  operaciones de personas siguen vetadas).
- **Riesgo residual:** un proceso con el mismo usuario de macOS que los runners puede leer los
  libros en disco y el keychain desbloqueado, y reconfigurar Tailscale. Mover los runners a otro
  usuario es decisión del usuario.

## Apéndice: referencias

- Hub: `~/otherwise-apps/CLAUDE.md`, `crates/helper/src/{lib,engine,runner,sched,state,mac}.rs`.
- otherwise-link: `docs/SPEC.md` §2.2 (locks, "otherwise-sync"), §4.4 (cursores), §8 (MCP),
  §11 (jobs, único escritor, lease); `crates/link/src/{lock,cloud,background,paths,registry}.rs`;
  `crates/hub/src/{lib,child}.rs`.
- otherwise-ui: `src/components/chrome.rs:122-160` (`Sidebar::header`).
- expense: `crates/core/src/{appsock,ops,mcp,ai_tools,lock,library,store,phone,email,
  stmt_inbox,background}.rs`; `docs/app-socket.md`, `docs/cli.md`;
  `docs/plans/2026-11-23-editions-personal-business.md`.
- expense me: `crates/me/src/{mcp,allow,edition,settings,cli}.rs`.
- Brújula: `crates/core/src/{store,oplog,hlc,mcp,registry,location}.rs`;
  `docs/ARCHITECTURE.md:5-7,71-108`; `docs/ROADMAP.md:3-21,61-62`.
- Vigía: `crates/core/src/{db,sync,background}.rs`, `crates/cli/src/{main,mcp}.rs`,
  `crates/app/src/state.rs`.
- Nodo, Mi Huerta, Surco, Voltia, Helio, CACFU, genealogy: los núcleos y `mcp.rs` citados en §3.2.
- Vcity: `docs/plans/2026-11-23-software-sales-vcity.md` (Licences on the desktop, Local data,
  Where things live on disk).
