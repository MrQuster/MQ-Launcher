# Журнал изменений (CHANGELOG)

Все заметные изменения проекта **MQ Launcher** документируются в этом файле.
Формат основан на [Keep a Changelog](https://keepachangelog.com/ru/1.0.0/), проект придерживается [Семантического версионирования](https://semver.org/lang/ru/).

---

## [v2.8.2] — 2026-11-25 ("Solid Edition — Stability & Cross-Platform Windows Parity")

### Исправлено и улучшено / Fixed and improved
- **Безопасность путей и поддержка кириллицы (`mq-ui`, `mq-launch`):**
  - Устранены паники срезов UTF-8 на кириллических и длинных путях (`C:\Пользователи\...`, `UNC`), защищён вызов системных обработчиков файлов и URL, устранена возможность инъекции флагов в `xdg-open`.
- **Авторизация Microsoft OAuth Device Code Flow (`mq-ui`, `mq-auth`, `mq-cli`):**
  - Реализован полноценный асинхронный вход с отображением кода подтверждения, кнопками копирования и перехода в браузер, корректной отменой и безопасным сохранением токенов в `AccountStore`.
- **Маскирование секретов (`mq-ui`, `mq-cli`, `mq-launch`):**
  - Все токены доступа, ключи API (CurseForge) и чувствительные аргументы командной строки строго маскируются `[REDACTED]` во всех логах, UI-консоли и отчётах об ошибках.
- **Discord Rich Presence и фоновые процессы (`mq-launch`, `mq-ui`):**
  - Устранён 60-FPS спам в IPC-канал Discord, очистка активности вызывается строго один раз при завершении игры; автоскрытие окна лаунчера при запуске (`hide_on_launch`) оптимизировано без холостых команд перерисовки.
- **Многопоточность и надёжность планировщика (`mq-sched`, `mq-mods`, `mq-install`):**
  - Безопасные бездедлочные каналы передачи команд между фоновыми воркерами и UI, корректная распаковка модпаков CurseForge с вычислением хешей SHA-1/SHA-512 через пул задач без блокировки потока рендеринга.
- **Парсер NBT и сервера (`mq-instances`):**
  - Оптимизированы алгоритмы чтения `servers.dat` и `level.dat`, безопасное пакетное чтение числовых массивов NBT, устранено зависание при парсинге повреждённых MOTD.

---

## [v2.8.1] — 2026-11-23 ("JVM Experimental Flags Fix & UI Cleanup")

### Исправлено и улучшено / Fixed and improved
- **Исправление аргументов запуска JVM (`mq-launch`, `mq-ui`):**
  - Исправлен краш запуска JVM (`VM option 'G1NewSizePercent' is experimental and must be enabled via -XX:+UnlockExperimentalVMOptions`) на Java 21+: `-XX:+UnlockExperimentalVMOptions` гарантированно предшествует `-XX:G1NewSizePercent`, `-XX:+ZGenerational` и другим экспериментальным флагам в рекомендациях оптимизатора, санитизации пользовательских аргументов и пресетах настроек.
  - Автоматическое исправление уже сохранённых конфигураций инстансов при запуске.
- **Интерфейс аккаунтов (`mq-ui`):**
  - Удалён экспериментальный 3D-просмотр скинов для повышения стабильности и чистоты кода при сохранении локальной библиотеки скинов и 2D-аватаров.

---

## [v2.8.0] — 2026-11-22 ("3D Skin Preview, Mod Profiles, World Backups & External Launcher Import")

### Добавлено и улучшено / Added and improved
- **3D-просмотр скинов и библиотека скинов (`mq-ui`, `mq-auth`):**
  - Реализован программный 3D-рендерер скинов и плащей на чистом `egui::Painter` с поддержкой скинов Steve (4px) и Alex (3px), плащей, плавной анимацией покачивания (walk animation), интерактивным вращением мышью (yaw/pitch) и пресетами ракурсов.
  - Добавлена локальная библиотека скинов с быстрым переключением, импортом PNG-скинов и управлением коллекцией.
- **Профили модов и бисекция крашей (`mq-mods`, `mq-instances`, `mq-ui`):**
  - Добавлена система именованных профилей модов (снапшотов включённых/выключенных модов) с быстрым сохранением, применением и переключением.
  - Реализован бинарный поиск виновника крашей (Mod Bisect Session) с пошаговым выявлением сбойных модов.
  - Добавлено автоисправление отсутствующих зависимостей модов и проверка обновлений модпаков с сохранением пользовательских файлов (сейвы, конфиги, скриншоты).
- **Миры, датапаки и автобэкапы (`mq-instances`, `mq-ui`):**
  - Добавлен парсинг метаданных `level.dat` (время последнего входа, размер, режим игры, хардкор, версия) для списка миров.
  - Реализовано управление датапаками конкретного мира (просмотр, включение, отключение, добавление ZIP-папок).
  - Добавлен настраиваемый автобэкап миров перед каждым запуском с ротацией копий.
- **Импорт сторонних лаунчеров (`mq-instances`, `mq-ui`):**
  - Добавлено автообнаружение и импорт инстансов из Prism Launcher, ElyPrism, MultiMC и Modrinth App со всеми модами, настройками и сохранениями.
- **Discord Rich Presence и оптимизация производительности (`mq-launch`, `mq-sched`, `mq-ui`):**
  - Реализована чистая интеграция Discord Rich Presence (IPC через сокеты домена / Windows named pipe) без внешних тяжеловесных библиотек с отображением статуса игры, времени и версии.
  - Добавлен мониторинг реального потребления ОЗУ (RSS) запущенной игрой с живым графиком (sparkline).
  - Добавлен автооптимизатор параметров JVM под конфигурацию ПК (Generational ZGC для Java 21+ на больших объёмах ОЗУ, настроенный G1GC).
  - Добавлены расширенные данные пинга серверов (MOTD с цветами `§`, фавиконки, игроки) и поддержка Quick Play (прямой вход в одиночный мир или сервер).

---

## [v2.7.6] — 2026-11-22 ("Forge 1.17–1.21+ Modpack Launch, Transparent Liquid Glass & UI Performance")

### Исправлено и улучшено / Fixed and improved
- **Запуск модпаков Forge 1.17–1.21+ / NeoForge 1.20.1–1.21+ (`mq-meta`, `mq-launch`, `mq-install`):**
  - Исправлен краш `java.lang.module.ResolutionException` (конфликт пакетов `net.minecraft.*` между модулем `minecraft` и vanilla `client.jar`) для `BootstrapLauncher` и `ForgeBootstrap`: `build_deterministic_classpath` больше не дублирует vanilla `client.jar` в `-cp` при FML-управляемом клиенте, а `ensure_ignore_list_covers_client_jar` гарантирует безопасное исключение `client.jar`/`<mc_version>.jar` в `-DignoreList`. / Fixed `ResolutionException` split-module crash on modular Forge/NeoForge launches by omitting vanilla `client.jar` from `-cp` for `BootstrapLauncher`/`ForgeBootstrap` and ensuring `-DignoreList` safely excludes it.
  - Исправлена ошибка `ERROR StatusLogger Reconfiguration failed: No configuration found for 'MCL'`: отключена инъекция устаревшего Mojang `logging.client` (`LegacyXMLLayout`) для модульного Forge/NeoForge и пропущенных XML-файлов; сохранён `id` в `FlexibleArtifact`. / Fixed Log4j2 `StatusLogger` error by skipping inherited Mojang `logging.client` on modular Forge/NeoForge and missing XML files, and preserving `id` in `FlexibleArtifact`.
  - Добавлена санитизация устаревших флагов GC (`sanitize_custom_jvm_args`): удаляется `-Xnoclassgc`, небезопасный `-XX:+UseSerialGC` автоматически повышается до `-XX:+UseG1GC` на больших/модпаковых кучах (`>= 3072 MB` или Java 17+), а ZGC-флаги защищены от несовместимых версий Java; пресет «Low-End PC (Low Memory)» переведён на безопасный G1GC. / Sanitized legacy JVM GC flags (stripping `-Xnoclassgc`, upgrading `-XX:+UseSerialGC` to G1GC on large/modded heaps, guarding ZGC flags by Java version) and updated the Low-End PC preset to safe G1GC.
  - Исправлены слияние нативных библиотек (`merge_libraries_deterministically`), замена legacy-аргументов Forge 1.12.2 (`merge_game_arguments_deterministically`), атомарное извлечение bundled `maven/` в `forge.rs` и проверка целостности `mq verify` / «Проверить» при пустом `AssetIndex.id` в сырых JSON-индексах Mojang. / Fixed native+artifact library merge deduplication, Forge 1.12.2 legacy argument replacement, atomic bundled Maven extraction, and `mq verify` false-missing asset index check when raw Mojang index JSON omits `"id"`.
- **Настоящее прозрачное стекло Liquid Glass (`mq-ui`):**
  - Полностью удалены встроенный фото-фон и автоподстановка обоев в теме `Liquid Glass`: режим `Procedural` оставляет окно по-настоящему прозрачным (`clear_color = [0, 0, 0, 0]`) для композитора Wayland/Hyprland/Windows, `Solid` рисует полупрозрачную стеклянную вуаль, а `CustomImage` загружает только явно выбранное пользователем изображение. / Removed baked wallpaper fallback from Liquid Glass: `Procedural` renders a truly transparent compositor window, `Solid` uses a subtle translucent glass veil, and `CustomImage` loads only an explicitly selected image.
  - Обновлён WGSL-шейдер `liquid_glass.wgsl` (`mode_params`, 128-байтное выравнивание uniform-буфера) и OpenGL/`glow`-fallback для полупрозрачного матового стекла с фаской, хроматической аберрацией и каустиками без непрозрачной подложки. / Updated `liquid_glass.wgsl` and OpenGL/`glow` fallback for translucent frosted glass cards over transparent windows.
- **Компоновка, отзывчивость UI и каталог (`mq-ui`):**
  - Кнопка «Играть» / «Скачать и играть» перенесена в панель действий слева от «Папка игры» в одну строку с выровненной высотой `32px`, а дублирующая «Установить» вынесена в меню `•••`. / Moved the Play button to the action toolbar strictly left of «Папка игры» at uniform 32px height, moving redundant Install into the `•••` menu.
  - Убрано техническое уведомление `Фаза 'catalog_search'`, локализованы пользовательские фазы, ускорен поиск каталога (пул `reqwest::Client` + таймаут 6 с на провайдера без 10-секундных зависаний) и исправлены хит-таргеты фильтров источника (`>= 24px`). / Suppressed raw `catalog_search` toast, accelerated catalog search with pooled HTTP clients and 6s per-provider timeout, and ensured `>= 24px` source filter hit targets.
  - Устранены фризы UI-потока (`WD`): виртуализация логов консоли (`show_rows`), кэширование проверок директорий/установки с TTL 2 с, перенос поиска Java, анализа крашей, пинга серверов, экспорта ZIP, удаления/переустановки и проверки обновлений в фоновый воркер, а также исправлены `mclogs`, OAuth Ely.by и изоляция консоли по инстансу. / Eliminated UI-thread stalls via virtualized console logs, 2s TTL directory caches, background worker offloading for Java discovery/crash analysis/pings/exports/updates, and fixed mclogs/Ely.by OAuth/per-instance console scoping.

---

## [v2.7.5] — 2026-11-22 ("Windows CI Test Parity & Uninstall Guard")

### Исправлено и улучшено / Fixed and improved
- **Удаление на Windows (`mq-update`):** в `execute_uninstall_with_roots` добавлена проверка `clean_exe.exists()` перед запуском отложенного процесса `cmd.exe` для самоудаления, чтобы не создавать лишние фоновые процессы для несуществующих или уже удалённых бинарников. / Guarded deferred Windows `cmd.exe` self-delete in `execute_uninstall_with_roots` with `clean_exe.exists()`.
- **Надёжность тестов и Windows CI (`mq-cli`, `mq-net`, `mq-sched`, `.github/workflows/ci.yml`):**
  - Исправлена нормализация путей в тесте `equivalence_and_stress` для Windows 8.3 short paths (`RUNNER~1`) и verbatim-префиксов (`\\?\`) с использованием `canonicalize_clean`. / Fixed Windows 8.3 short-path and verbatim `\\?\` normalization in `equivalence_and_stress` tests.
  - Увеличен запас таймеров в тестах конкурентной дедупликации (`mq-net`), отмены задач/DAG и watchdog (`mq-sched`) с учётом квантования таймеров Windows (`15.6ms`); включён `--no-fail-fast` для Windows-тестов в CI. / Hardened timer margins in `mq-net` and `mq-sched` tests for Windows timer quantization and enabled `--no-fail-fast` in Windows CI tests.

---

## [v2.7.4] — 2026-11-22 ("Rust 1.100 Clippy & Modrinth Cache Reliability")

### Исправлено и улучшено / Fixed and improved
- **Совместимость с Rust 1.100 Clippy (Windows и Linux) / Rust 1.100 Clippy Compatibility (`mq-launch`, `mq-cli`, `mq-mods`):**
  - Устранены предупреждения `clippy::needless_bool` в `mq-launch/src/java.rs` (`is_executable_file` на ветках `unix`/`not(unix)`), `mq-cli/src/main.rs` (`should_attach_parent_console`) и `mq-mods/src/scanner.rs` (`is_dependency_satisfied`) для чистой сборки с `-D warnings` на Windows и Linux. / Eliminated Rust 1.100 `clippy::needless_bool` lints across `java.rs`, `main.rs`, and `scanner.rs` so both Windows and Linux CI pass cleanly with `-D warnings`.
- **Дисковый кэш Modrinth API / Modrinth Disk Cache Performance & Reliability (`mq-mods`):**
  - Запись временных файлов дискового HTTP-кэша в `mq-mods/src/modrinth.rs` переведена на уникальные соседние `.tmp.<pid>.<seq>` файлы с атомарным переименованием через `rename_with_retry` без блокирующего `fsync` (`sync_all`), что ускоряет кэширование и устраняет таймауты на нагруженных дисках CI; увеличен запас TTL в тесте кэширования с ETag. / Updated Modrinth HTTP disk cache writes to atomic sibling temp-file renames via `rename_with_retry` without blocking `fsync`, eliminating stalls on loaded disks and hardening the ETag cache TTL test.

---

## [v2.7.3] — 2026-11-21 ("Full Windows Compatibility & Reliability")

### Исправлено и улучшено / Fixed and improved
- **Установка и запуск Minecraft на Windows / Windows Minecraft Install & Launch (`mq-meta`, `mq-install`, `mq-launch`, `mq-instances`, `mq-mods`):**
  - Исправлен критический баг выбора нативных библиотек: `mq-install` теперь использует `RuleContext::current()` вместо жёсткого Linux-профиля, распознаёт и извлекает Windows `.dll` (`natives-windows-${arch}`, `windows/x64/*.dll`), и корректно оценивает Mojang-правила OS/arch/version для Windows. / Fixed hardcoded Linux `RuleContext` in installer, added `.dll` native extraction with `${arch}` expansion and full Windows Mojang rule evaluation.
  - Исправлен критический баг аргументов запуска на Windows: `-Dfile.encoding=UTF-8` перенесён до `main_class` (ранее падал JOptSimple на каждом запуске); убраны мигающие окна консоли (`CREATE_NO_WINDOW`) во всех дочерних процессах Java/Forge/`taskkill`; добавлено автоматическое сжатие classpath и JEP 293 `@argfile` для Java 9+ при обходе лимита `CreateProcessW` 32 767 символов (без записи `--accessToken` на диск); потоковое чтение логов теперь устойчиво к CP1251/CP866 и CRLF. / Moved `-Dfile.encoding=UTF-8` before `main_class`, suppressed console flashes via `CREATE_NO_WINDOW`, added Windows 32 KB command-line compaction + Java 9+ `@argfile` offloading (keeping secrets off disk), and made log streaming resilient to CP1251/CRLF.
  - Расширен автопоиск Java на Windows (Adoptium, Microsoft, Zulu, BellSoft, Corretto, GraalVM, Scoop, Chocolatey, MSI/UWP Minecraft Launcher с приоритетом `java.exe`), добавлен fallback копирования вместо симлинков без прав администратора в `mojang_java`, очистка префиксов `\\?\`, защита от зарезервированных имён Windows (`CON`, `NUL`, `COM1`…) и Zip-Slip с `\`, а также определение GPU через Win32 `EnumDisplayDevicesW`. / Expanded Windows Java discovery, added non-admin symlink-to-copy fallback, stripped `\\?\` verbatim prefixes, blocked Windows reserved instance names & backslash Zip-Slip, and replaced `glxinfo` with Win32 `EnumDisplayDevicesW`.
- **Телеметрия, файловая система, автообновление и OAuth на Windows / Windows Telemetry, FS, Self-Update & Auth (`mq-sched`, `mq-core`, `mq-update`, `mq-auth`, `mq-net`):**
  - Заменены Linux-зависимости `/proc/stat`, `/proc/self/status`, `/proc/self/fd` на кроссплатформенные Win32 API (`GetSystemTimes`, `K32GetProcessMemoryInfo`, `GetProcessHandleCount`) и `sysinfo`. / Replaced `/proc/*` telemetry with Win32 `GetSystemTimes`, `K32GetProcessMemoryInfo`, `GetProcessHandleCount`, and `sysinfo`.
  - Устранена потеря данных при замене заблокированных файлов на Windows (безопасный backup-swap с откатом в `rename_with_retry`), добавлено снятие атрибута `ReadOnly` и ретраи при блокировках Windows Defender в `mq-net`/`mq-core`/`mq-auth`. / Made atomic file replacement rollback-safe under Windows file locks & Defender scans with read-only clearing.
  - Исправлено несоответствие бэкапа `.old` (`mq.exe.old` vs `mq.old`) в `mq-update`, строгий подбор релизных ассетов по платформе, разбор `SHA256SUMS` с UTF-8 BOM/CRLF, нативные Win32 `ShellExecuteW` (сохраняет `&` в OAuth URL без `cmd /c start`) и `MessageBoxW` (без PowerShell/CP1251-кракозябр), а также защита loopback-слушателя Ely.by от преконнектов и `/favicon.ico`. / Fixed `mq.exe.old` rollback/cleanup mismatch, strict platform asset matching, BOM/CRLF `SHA256SUMS`, native Win32 `ShellExecuteW` & `MessageBoxW`, and hardened Ely.by OAuth loopback listener.
- **GUI, Liquid Glass WGPU/DX12, шрифты и анимации / Windows GUI, DX12 Liquid Glass, Fonts & Animation Fix (`mq-ui`):**
  - Исправлен `RwLock`-дедлок `ctx.cumulative_pass_nr()` внутри `ctx.data_mut()` в `animation.rs` при отключённых анимациях (`MQ_TEST_DETERMINISTIC=1` / `animation_time == 0.0`). / Fixed `RwLock` deadlock in `animation.rs` when animations are disabled.
  - Усилена совместимость WGSL-шейдера `Liquid Glass` с DX12/HLSL (`safe_pow`, строгие массивы), защита от паник при свёрнутом окне `0x0` и дробном Windows HiDPI (125%/150%/175%), адаптивные лимиты `wgpu` с автоматическим откатом `WGPU -> OpenGL/Glow`, нативный выбор системных Windows-шрифтов с кириллицей, нативное открытие URL/папок через `ShellExecuteW`, корректная папка `Downloads` (`FOLDERID_Downloads`) и скрытие Linux-обёрток (`gamemoderun`/`mangohud`) на Windows. / Hardened DX12 Liquid Glass shader, prevented minimized `0x0` & fractional HiDPI scissor/clamp panics, added automatic `WGPU -> Glow` fallback, Windows Cyrillic system font discovery, `ShellExecuteW` opener, `FOLDERID_Downloads`, and Windows-appropriate UI labels.
- **CLI, манифест `.exe`, установщики и CI / Windows CLI, Manifest, Installers & CI (`mq-cli`, `install.ps1`, `packaging/`, `.github/workflows/`):**
  - Встроен Windows application manifest (`longPathAware`, `PerMonitorV2` DPI, `UTF-8`), улучшено подключение к родительской консоли для CLI-команд, исправлены `mq doctor` / `mq bench` на Windows (`sysinfo` + `GetDiskFreeSpaceExW`). / Embedded Windows app manifest (`longPathAware`, `PerMonitorV2`, UTF-8), hardened parent console attach, fixed `mq doctor` and `mq bench` on Windows.
  - Исправлены `install.ps1`, Inno Setup `mq.iss` (`addtopath` с сохранением `REG_EXPAND_SZ`, безопасный поиск деинсталлятора по AppId/Publisher), унифицирован плоский portable ZIP (`mq.ico`, Scoop `mq.json`, Winget), а также добавлен Windows `clippy -D warnings` и проверки манифеста в CI/Release Pipeline. / Hardened `install.ps1`, Inno Setup PATH handling (`REG_EXPAND_SZ`), unified flat portable ZIP/Scoop/Winget packaging, and added Windows clippy + manifest checks in CI.

---

## [v2.7.2] — 2026-11-19 ("Downloads, Performance & Mod Conflicts")

### Добавлено и улучшено / Added and improved
- **Вкладка «Загрузки» / Downloads tab (`downloads_view.rs`):** активные установки, очередь модов и сборок с этапами, скоростью и ETA, ручные загрузки, график скорости и история сессии. / Active installs, content/modpack queue with stages, speed and ETA, manual downloads, live speed chart and session history.
- **Производительность и диагностика / Performance live telemetry & diagnostics (`perf_view.rs`, `hud.rs`):** CPU, память, время кадра, нагрузка по ядрам, запущенные игры, настройки и статистика диска/кэша с очисткой. / CPU, memory, frame time, per-core load, running games, tuning, storage stats and temp cache cleanup.
- **Конфликты модов / Mod conflict detection (`mq-mods/scanner.rs`):** исправлен разбор Forge 1.7.10–1.12.2 `mcmod.info`, `mods.toml`/NeoForge, Fabric/Quilt (`provides`, `breaks`, optional), Jar-in-Jar и `MANIFEST.MF`; новый тип `IncompatibleMod`; кнопка «Найти в каталоге» для недостающих зависимостей. / Fixed Forge 1.12.2 `mcmod.info`, NeoForge, Fabric/Quilt, JiJ and `MANIFEST.MF` parsing; new `IncompatibleMod`; "Find in catalog" for missing dependencies.

---

## [v2.7.1] — 2026-11-19 ("Smooth Spring Animations & UI Transitions")

### Добавлено и улучшено
- **Единый движок анимаций (`mq-ui/src/animation.rs`):**
  - Добавлена физика затухающей пружины (`SpringConfig`, `step_spring_1d`, `animate_spring_*`) и easing-кривые (`ease_out_cubic`, `ease_out_expo`, `ease_in_out_cubic`, `ease_out_quad`) с хранением состояния в `egui::Memory` по `egui::Id`.
  - Реализован умный планировщик перерисовки: `ctx.request_repaint()` вызывается **только пока активна хотя бы одна анимация**, обеспечивая нулевую нагрузку на CPU/GPU в покое.
- **Плавное переключение вкладок и экранов (`sidebar.rs`, `app.rs`, `tabs.rs`, `segmented.rs`, `accounts_view.rs`, `settings_view.rs`, `catalog_view.rs`, `instances_view.rs`):**
  - Реализованы плавно скользящие индикаторы активной вкладки (`animate_spring_rect`) в боковом меню, подчеркивающих вкладках деталей инстанса, `segmented_switch`, настройках, аккаунтах, каталоге и подрежимах контента.
  - Добавлен переход между экранами вкладок (`render_tab_transition`): плавное проявление (fade-in), направленный сдвиг на 14 px в сторону переключения и лёгкий масштаб `0.985 -> 1.0` без джиттера верстки и с учётом `TSTransform` в WGSL-шейдере `Liquid Glass`.
- **Микро-анимации, окна, выпадающие меню, прогресс-бары и уведомления (`widgets.rs`, `components/`, `catalog_dialogs.rs`, `theme_styles.rs`, `liquid_glass.wgsl`):**
  - Добавлены плавный отклик при наведении, мягкое сжатие при клике и плавная подсветка для кнопок, карточек, чипов, строк списков и переключателей.
  - Все модальные окна и выпадающие меню плавно открываются (fade-in + масштаб `0.95 -> 1.0` + плавное затемнение фона) и не исчезают мгновенно при закрытии, дожидаясь завершения анимации закрытия (`animate_modal_presence`, `animate_modal_cached`, `show_animated_modal_window`, `show_animated_dropdown_popup`).
  - Добавлены пружинная интерполяция прогресс-баров и менеджер всплывающих уведомлений `ToastManager` с выездом, плавной перестановкой и исчезновением.

---

## [v2.7.0] — 2026-11-18 ("Hardware-Accelerated WGPU Liquid Glass Shader")

### Добавлено и улучшено
- **Аппаратный WGSL-шейдер Liquid Glass (`mq-ui/src/shaders/liquid_glass.wgsl`, `theme.rs`, `theme_styles.rs`, `app.rs`, `snapshot.rs`):**
  - Добавлен фрагментно-вершинный WGSL-шейдер для `egui 0.31` + `wgpu 24` через `egui_wgpu::CallbackTrait` (`PaintCallback`): аналитическая SDF-маска скруглённого прямоугольника (`sd_rounded_box`), преломление фаски Снеллиуса, RGB хроматическая аберрация, адаптивная под яркость обоев матовая вуаль и направленные каустические блики, реагирующие на позицию курсора.
  - Перенесены резкая и GPU-размытая текстуры обоев в `egui_wgpu::CallbackResources` (`LiquidGlassGpuResources`) с 4-проходным separable GPU blur, обновляемым только при смене обоев или ресайзе окна.
  - Реализован динамический uniform-буфер с выравниванием по `min_uniform_buffer_offset_alignment` и масштабирование координат через `pixels_per_point()`.
  - Сохранён безопасный fallback на `egui::Mesh` + CPU-блюр при запуске без Vulkan/wgpu (`--backend glow` / OpenGL и unit-тесты).

---

## [v2.6.10] — 2026-11-18 ("Serpantinum 1:1 Liquid Glass & Automatic CurseForge CDN Downloads")

### Добавлено и улучшено
- **Liquid Glass (Serpantinum 1:1) (`mq-ui/theme_styles`, `theme`, `app`, `settings_view`, `mq-core`):**
  - Преломление размытых фоновых обоев по закону Снеллиуса на 10px скруглённых фасках (`LENS_ZOOM = 0.045`), тонкий 1px направленный серебристо-белый кант (`lightDir = normalize(-0.55, -0.83)`) и нейтральная тёмно-угольная матовая глазурь вместо мутной синей сетки и больших курсорных многоугольников.
  - Полноэкранная отрисовка обоев (`CustomImage` и автоматический фолбэк `~/.cache/serpantinum/wallpaper/current_wallpaper.png`) сквозь сайдбар, нижний HUD и рабочую область.
  - Раздельные компактные стеклянные карточки во вкладке «Кастомизация и Тема» и центрированный логотип с двухколоночными плитками во вкладке «О программе».
- **Автоматическое скачивание модов и сборок CurseForge (`mq-mods/curseforge`, `cfpack`, `manager`, `resolver`):**
  - Все моды и модпаки CurseForge теперь скачиваются автоматически напрямую через `edge.forgecdn.net` / `mediafilez.forgecdn.net` (с поддержкой кодирования спецсимволов и вариантов `+` -> `%2B`) даже при `downloadUrl: null` или `allowModDistribution: false` без диалога ручной загрузки.

---

## [v2.6.9] — 2026-11-18 ("Apple iOS 26 Liquid Glass & Translucent Themes")

### Добавлено и улучшено
- **Оптический движок Apple iOS 26 Liquid Glass (`mq-ui/theme_styles`, `theme`, `app`, `sidebar`, `hud`):**
  - Бесшовный анимированный полноэкранный фон авроры на `LayerId::background()` под боковой панелью, нижним HUD и рабочей областью.
  - Выпуклая линзовая рефракция фона внутри карточек, кнопок, капсул навигации, переключателей и полей поиска (`build_refracted_glass_body_mesh`, `collect_glass_underlay_shapes`) с верхним купольным бликом, нижним внутренним каустиком, внешней тенью-левитацией и плавным курсорным бликом (`cursor_specular_glow`).
  - Двойная анизотропная грань Френеля и внутренний каустический контур (`paint_refractive_gradient_border`), капсульные контролы и теги (`.capsule`), концентрические скругления карточек и светящийся ореол выбранных элементов.
- **Чистая тема «Полупрозрачная» (`Translucent`):**
  - Добавлена лёгкая полупрозрачная тонировка с прозрачностью окна на уровне композитора/фреймворка (`with_transparent(true)` и `clear_color`) без лишних бликов и с высокой читаемостью текста.
- **Переключатель тем и прозрачности (`mq-core/config`, `mq-ui/settings_view`):**
  - Добавлены пресеты `Liquid Glass` и `Полупрозрачная` с сохранением в конфиге, выбором темы и слайдером прозрачности окна в настройках лаунчера.

---

## [v2.6.8] — 2026-11-18 ("CI Smoke Stabilization, Updated Demo Screenshots & Packaging Sync")

### Исправлено и улучшено
- **UI и детерминизм каталога (`mq-ui`):**
  - Включена пост-QA доработка детерминированного режима каталога (`is_deterministic_test_mode`) и перегенерированы актуальные демо-скриншоты интерфейса в `docs/assets/screenshots/` по состоянию после QA-исправлений.
- **Стабильность headless CI:**
  - Зафиксирована установка системных зависимостей `libglx-mesa0` и `libxkbcommon-x11-0` для стабильного запуска Xvfb + `--backend glow` в GitHub Actions.
- **Синхронизация пакетных манифестов (`packaging/`):**
  - Обновлены версии и ссылки релиза до `2.6.8` в `PKGBUILD`, Scoop (`packaging/scoop/mq.json`), Winget (`packaging/winget/`), Inno Setup (`packaging/windows/mq.iss`) и примерах `install.ps1`.

---

## [v2.6.7] — 2026-11-15 ("Autonomous QA Hardening, Loader/Mod Fixes & CI Smoke")

### Исправлено и улучшено
- **UI/UX и доступность (`mq-ui`):**
  - Исправлена отмена загрузки инстанса (`CancelDownload`), закрытие модальных окон по `Esc`, видимость кнопок диалога удаления при малом разрешении и масштабе `2.0x`, перевод `AccountsView` на английский язык и контрастность активных вкладок каталога (`>= 4.5:1`).
  - Добавлена детерминированная очистка результатов при переключении источника каталога и пропуск автопоиска в детерминированном режиме.
- **Загрузчики и установка (`mq-install`, `mq-meta`):**
  - Улучшена поддержка NeoForge, локального импорта Forge JAR в CAS и обнаружения Java в `MQ_DATA_DIR`.
- **Моды, ресурспаки и шейдеры (`mq-mods`, `mq-cli`):**
  - Добавлены preflight-проверки зависимостей, отслеживание `required_by` / orphan-модов, распознавание `fabric-api` / `quilted_fabric_api` для подмодулей `fabric-*` и запись `.mq-lock.json` в `mq mods install`.
- **Запуск игры и отказоустойчивость (`mq-launch`, `mq-core`):**
  - Добавлены отслеживание PID процесса игры, завершение дерева процессов, коды выхода по сигналам, расширенные паттерны анализатора крашей и очистка устаревших `.part`/`.tmp` файлов при старте (`PERF-01`).
- **Инфраструктура QA и CI:**
  - Интегрирован изолированный QA-харнес (`--features test-hooks`, строго отключён в release-сборках), сценарии `qa/scenarios/*.yaml`, `scripts/verify.sh` и автоматический headless-прогон `smoke.yaml` (Xvfb + `--backend glow`) в GitHub Actions CI.

---

## [v2.6.6] — 2026-11-14 ("UI/UX Polish, Cyrillic Sizing & Live QA Fixes")

### Исправлено и улучшено
- **Корректная ширина виджетов для кириллицы (`mq-ui/components`):**
  - Исправлен расчёт ширины текста в `underline_tabs`, `segmented_switch`, `button_primary`, `button_secondary`, `Select` и `FilterChip`: вместо длины в байтах UTF-8 (`.len()`) теперь измеряется фактическая геометрия шрифта (`layout_no_wrap`), благодаря чему вкладки (`Консоль`, `Моды`, `Ресурспаки`, `Шейдеры`, `Миры`, `Серверы`, `Скриншоты`, `Параметры`), кнопки и фильтры больше не растягиваются вдвое и помещаются без обрезки.
- **Сетка инстансов (`InstancesView` Grid View):**
  - Кнопка запуска на карточке инстанса в режиме **Сетка** теперь реально запускает/скачивает или останавливает инстанс; добавлена прямая кнопка **Моды**, компактная кнопка папки и открытие карточки по двойному клику.
- **Контент внутри инстанса и синхронизация каталога:**
  - Исправлено выравнивание названий модов, ресурспаков и шейдерпаков (убрано искусственное центрирование в широком блоке).
  - Компактная панель переключения **Установленные / Каталог** помещается в одну строку.
  - Индикатор «Установлен» / «Уже добавлен» в каталоге теперь корректно проверяет не только моды, но и каталоги `resourcepacks/` и `shaderpacks/` выбранного инстанса.
  - Завершение задач загрузки/установки теперь сканирует целевой инстанс из результата задачи и отображает статус в футере (включая ошибки).
- **Удаление лаунчера на Linux и Windows (`mq uninstall`, GUI и скрипты установки):**
  - Добавлена кроссплатформенная команда `mq uninstall` (с поддержкой `--dry-run`, `--purge` и `-y/--yes`), которая удаляет бинарник, системные ярлыки/иконки (на Windows дополнительно запускает тихий `unins000.exe` и отложенное удаление `mq.exe`), сохраняя инстансы, моды и миры, если не передан `--purge`.
  - В **Настройки → Обновления** и **Настройки → О программе** добавлен блок удаления лаунчера с двухшаговым подтверждением, опцией полной очистки данных и копированием команды для терминала / PowerShell.
  - Обновлены `install.sh` (`--uninstall`, `--purge`, `MQ_UNINSTALL=1`, `MQ_PURGE=1`) и `install.ps1` (`-Uninstall`, `-Purge`, очистка `%LOCALAPPDATA%\Programs\MQ`, ярлыков и пользовательского `PATH`).
- **Диагностика `mq doctor`:**
  - Актуализирована версия (`CARGO_PKG_VERSION`) и передача заголовка `x-api-key` при проверке `CurseForge API`.

---

## [v2.6.5] — 2026-11-14 ("In-Instance Mods, ResourcePacks & Shaders Installation")

### Добавлено и изменено
- **Установка контента прямо внутри инстанса (`Моды`, `Ресурспаки`, `Шейдеры`):**
  - Вкладки **Моды**, **Ресурспаки** и **Шейдеры** внутри инстанса получили встроенный переключатель режима **Установленные / Каталог (Modrinth / CurseForge)** и кнопку быстрого скачивания (`Скачать моды`, `Скачать ресурспаки`, `Скачать шейдеры`).
  - Встроенный каталог внутри инстанса автоматически привязывается к текущему инстансу, его версии Minecraft и мод-лоадеру (`CatalogState::sync_for_instance`, `CatalogView::ui_for_instance_tab`) и позволяет устанавливать моды (с разрешением зависимостей и пакетной очередью), ресурспаки и шейдерпаки без ухода со страницы инстанса.
  - Добавлена быстрая установка локальных файлов (`.jar` для модов, `.zip` для ресурспаков и шейдерпаков) по пути или через перетаскивание в окно (`install_local_content_file`).

---

## [v2.6.0] — 2026-11-14 ("Out-of-the-Box CurseForge Mods & Rounded mq Icon")

### Добавлено и изменено
- **Встроенный ключ CurseForge API v1 из коробки:**
  - Встроена поддержка скачивания модов, ресурс-паков, шейдеров и сборок с CurseForge сразу после установки без необходимости вручную получать персональный API-ключ (`mq_core::builtin_cf_api_key`, fallback в `effective_cf_api_key` и `CurseForgeClient::from_env`, автоматическое маскирование в логах через `sanitize_cf_api_key`).
  - Возможность указать свой личный ключ в **Настройки → Каталог** или переменной `MQ_CURSEFORGE_API_KEY` полностью сохранена и имеет приоритет.
  - В CI/Release-пайплайн добавлена передача секрета `MQ_CURSEFORGE_API_KEY` при сборке релизных бинарников для Linux и Windows.

---

## [v2.5.1] — 2026-11-14 ("Redesigned Rounded mq Ligature Icon")

### Изменено
- **Новая скруглённая иконка с лигатурой `mq`:**
  - Перерисован фирменный знак в виде мягкого скруглённого медальона (`rx="88"`) с плавно соединёнными строчными буквами `mq`: `docs/assets/logo.svg`, `docs/assets/logo-light.svg`, `docs/assets/social-preview.png`, `packaging/mq.svg`, `packaging/windows/mq.ico`, `packaging/windows/wizard-small.bmp`, `packaging/windows/wizard-large.bmp` и внутри приложения (`crates/mq-ui/src/sidebar.rs`: `paint_mq_emblem`, `build_app_icon`).
  - Обновлены скриншоты интерфейса в `docs/assets/screenshots/`.

---

## [v2.5.0] — 2026-11-14 ("Calm Palette, Customization Studio & Batch Mod Queue")

### Изменено и добавлено
- **Спокойная палитра и контрастность интерфейса:**
  - Заменены яркие кислотные акценты на сбалансированную палитру (`#4C9A82` / `#2E9E6B` и `LightClean` `#059669`), обеспечена чёткая читаемость диалога создания инстанса, списка версий Minecraft, треков слайдеров и кнопок в тёмной и светлой темах.
- **Расширенная кастомизация и процедурный фон:**
  - Добавлен выбор стиля фона (`Procedural` с архитектурной сеткой/градиентом, `Solid`, `CustomImage` с cover-кропом и затемнением), спокойные пресеты акцентов (`CALM_ACCENT_PRESETS`) и произвольный цвет, а также настройка прозрачности, обводки, скруглений, масштаба UI, шрифта и плотности с применением без перезапуска.
- **Эргономика модов и пакетная установка в каталоге:**
  - Улучшен просмотр и фильтрация модов/ресурспаков/шейдеров внутри инстанса и массовое обновление; в каталоге добавлена очередь пакетной установки (`batch_queue`) с выбором целевого инстанса и дедуплицированным разрешением общих зависимостей.
- **Минималистичная графика и документация:**
  - Переработан геометрический знак `MQ` в приложении, SVG-логотипах (`logo.svg`, `logo-light.svg`, `packaging/mq.svg`), ресурсах Windows-установщика (`mq.ico`, `wizard-small.bmp`, `wizard-large.bmp`) и `social-preview.png`; обновлены `README.md`, `README.en.md` и скриншоты.

---

## [v2.4.0] — 2026-11-14 ("Clean UI, Windows Installer & Graphify")

### Добавлено
- **Полноценная дистрибуция для Windows 10/11 x64:**
  - Мастер установки Inno Setup 6 (`packaging/windows/mq.iss`) с графитовыми BMP-ассетами (`wizard-large.bmp`, `wizard-small.bmp`), установкой без UAC в `%LOCALAPPDATA%\Programs\MQ`, языками RU/UK/EN, ассоциацией `.mrpack`, безопасным закрытием только `mq.exe` и удалением с расчётом размера данных (`/PURGE=1` для тихого режима).
  - Обновлённый PowerShell-установщик (`packaging/windows/install.ps1`) с проверкой TLS 1.2+, `SHA256SUMS` и Minisign, флагами `-Version`, `-Portable`, `-Silent`, `-Uninstall`, `-BaseUrl`.
  - Портативный режим через `portable.flag` (`<exe_dir>/data/` без записи в реестр), запуск `mq.exe <file.mrpack>` с передачей пути запущенному окну по loopback IPC, автообновление через `MQ-Setup-<version>-x64.exe` при наличии `unins000.exe` (или атомарную подмену в portable-режиме).
  - Статическая CRT (`+crt-static`), встроенные `mq.ico` и `VERSIONINFO` (`crates/mq-cli/build.rs`), манифесты `winget` и `scoop`, документация `docs/WINDOWS_SIGNING.md` и `docs/WINDOWS_TEST_CHECKLIST.md`, CI/Release-пайплайн с автотестами PowerShell на `windows-latest`.
- **Чистый интерфейс и визуальный QA:**
  - Убраны шаблоны быстрого создания с хардкодом `1.21.1` и технические плашки; добавлен чистый Empty State «Инстансов пока нет» и обновлённый диалог «Создать инстанс» с манифестом Mojang (`latest.release`, фильтры, `26.x`).
  - Обновлена боковая панель (`224 px` / компактный режим `64 px`, авто-сворачивание `<1000 px`), сгруппированные разделы, блок аккаунта с усечением ника, единый набор векторных иконок Lucide `16 px` (добавлена лицензия ISC в `THIRD_PARTY_LICENSES.md`) и клавиатурная навигация (`Ctrl+1..6`, стрелки, `Enter`).
  - Проведён двухэтапный визуальный QA (`docs/VISUAL_QA_REPORT.md`), обновлены скриншоты в `docs/assets/screenshots/`.
- **Инструментарий разработки:**
  - Настроен `.graphifyignore` и проверка качества `scripts/verify.sh` (`fmt`, `clippy -D warnings`, `cargo test --workspace --locked`, проверка заглушек и ключей).

---

## [v2.3.0] — 2026-11-14 ("Unified Modrinth + CurseForge Catalog")

### Добавлено
- **Единый каталог Modrinth API v2 и CurseForge API v1:**
  - Поддержка четырёх типов контента: моды, ресурспаки, шейдеры и сборки (модпаки) в одном экране «Каталог».
  - Переключатель источника («Все», «Modrinth», «CurseForge»), поиск с задержкой 300 мс и отменой устаревших запросов, фильтры по версии Minecraft (включая `26.x`), загрузчику, категориям и сортировке, пагинация, ленивая загрузка и кэширование иконок, скелетоны загрузки.
  - Страница проекта с безопасным рендерингом Markdown/HTML, списком версий и диалогом установки с выбором инстанса и необязательных зависимостей.
  - Личный ключ CurseForge в **Настройки → Каталог** (хранение с правами `0600` или `MQ_CURSEFORGE_API_KEY`); без ключа CurseForge показывает инструкцию, а Modrinth продолжает работать полностью.
  - Диалог ручной загрузки с отслеживанием папки загрузок и перетаскиванием для файлов CurseForge, где автор запретил автоматическую раздачу (`downloadUrl == null`, без обхода через неофициальные CDN).
  - Обновление установленного контента в один клик с резервным копированием `*.bak` и автоматическим откатом при ошибке, плюс бейджи источника из `mods/.mq-lock.json`.
  - Импорт локальных сборок `.mrpack`, CurseForge `.zip` и MultiMC/Prism (через UI, drag-and-drop и CLI `mq import`), с защитой от Zip-Slip, симлинков и zip-бомб и игнорированием pre/post-launch команд.
  - Новые CLI-команды `mq search`, `mq install modrinth:<slug>` / `curseforge:<id>` и `mq import`.

---

## [v2.2.0] — 2026-11-07 ("Solid Edition — Full Windows 10/11 & Linux Parity")

Релиз **MQ v2.2.0 "Solid Edition"** обеспечивает полный функциональный паритет лаунчера для операционных систем Windows 10/11 x64 и Linux, надежную кроссплатформенную установку в одну команду, криптографическую защиту цепочки поставок через цифровые подписи Minisign Ed25519, аттестации Sigstore и авторизацию Ely.by через браузер по протоколу OAuth2 + PKCE S256.

### Добавлено
- **Полный паритет для Windows 10/11 x64:**
  - Кроссплатформенные пути данных (`%LOCALAPPDATA%\MQ` на Windows, `~/.local/share/mq` и `~/.config/mq` на Linux).
  - Автоматическое определение разделителя classpath (`;` на Windows, `:` на Linux).
  - Разрешение правил манифеста Minecraft для `natives-windows` (`RuleContext::windows_x86_64()`).
  - Флаг создания процессов `CREATE_NO_WINDOW` (`0x08000000`) для подавления паразитных консольных окон JVM.
  - Надежное завершение дерева процессов через `taskkill /PID <pid> /T /F` на Windows.
  - Поиск и автоматическая загрузка Adoptium Temurin OpenJDK в формате `.zip` с распаковкой на Windows.
  - Системные шрифты Windows (Segoe UI, Consolas) и корректная поддержка Per-Monitor DPI.
  - Безопасная работа с путями, содержащими пробелы, кириллицу и префикс длинных путей `\\?\`.
  - Портативный режим работы через маркер `portable.flag` (изоляция всех данных рядом с `mq.exe`).
- **Установка в одну команду и дистрибуция:**
  - Скрипт 1-командной установки для Linux: `curl -fsSL https://github.com/MrQuster/mq/releases/latest/download/install.sh | sh`.
  - Скрипт 1-командной установки для Windows PowerShell: `irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1 | iex`.
  - Установщик Inno Setup (`MQ-Setup-2.2.0.exe`) с установкой per-user (`%LOCALAPPDATA%\Programs\MQ`, без требования прав администратора UAC) и поддержкой тихого обновления `/VERYSILENT`.
  - Портативный архив `mq-windows-x86_64-portable.zip` для запуска с USB-накопителей.
  - Манифесты пакетов для Windows Package Manager (`winget`) и Scoop (`packaging/scoop`).
- **Безопасность и целостность цепочки поставок (Supply Chain Integrity):**
  - Цифровая подпись релизов Ed25519 Minisign (`SHA256SUMS.minisig`) с официальным публичным ключом:
    `RWRlxrUOJgeLq7AIndJRG9ZhsdT5QiHy9usykXvxt1C49a1h/km3h9lt`.
  - Интеграция проверки Minisign в подсистему самообновления лаунчера (`crates/mq-update`).
  - Защита от даунгрейда (отказ от установки версий ниже текущей).
  - Аттестация сборки бинарных файлов через GitHub OIDC Sigstore (`actions/attest-build-provenance`).
  - Комплексная модель угроз (`docs/THREAT_MODEL.md`) и политика безопасности (`SECURITY.md`).
  - Политика строгой нулевой телеметрии и реестр внешних доменов (`docs/PRIVACY.md`).
  - Санитизация импортируемых сборок: автоматическое отключение `wrapper_cmd` и скриптов `PreLaunchCommand`/`PostExitCommand` из MultiMC/Prism архивов с предупреждением в логах.
- **Авторизация Ely.by по стандарту OAuth2 + PKCE S256:**
  - Полный переход на браузерный вход через `oauth2_browser_authorize_url_pkce` с публичным `client_id = "mq-launcher"`.
  - Генерация криптографических PKCE пар verifier/challenge (RFC 7636) методом S256.
  - Локальный HTTP loopback-сервер (`127.0.0.1:<port>/callback`) с валидацией CSRF-токена и автоматическим закрытием через 5 минут.
  - Автоматическое обновление токенов через `/api/oauth2/v1/token` (`grant_type=refresh_token`).
  - Загрузка и инъекция актуальной версии `authlib-injector` (`-javaagent`) с проверкой SHA-256.

---

## [v2.1.0] — 2026-11-07 ("Solid Edition")

Релиз **MQ v2.1.0 "Solid Edition"** сосредоточен на максимальной надежности, абсолютной монолитности интерфейса (Solid Dark, отказ от полупрозрачности и AI-slop эффектов), безопасной авторизации через браузер, системе самообновления в один клик и полной поддержке версий Minecraft линейки `26.x`.

### Добавлено
- **Подсистема самообновления (`crates/mq-update`):**
  - Автоматическая проверка новых релизов через GitHub Releases API.
  - Потоковая верификация целостности SHA-256 по манифесту `SHA256SUMS`.
  - Атомарная замена исполняемого файла через `<exe>.new` со smoke-тестом перед подменой.
  - Механизм быстрого отката (rollback) к предыдущей версии `<exe>.old`.
  - Перезапуск процесса в новой сессии через `setsid` с аргументом `--after-update`.
  - Защита системных установок: обнаружение путей `/usr/` с подсказкой об обновлении через системный пакетный менеджер.
  - Команда CLI `mq self-update [--check] [--prerelease]`.
- **Авторизация Ely.by через браузер (`OAuth2 Loopback`):**
  - Полный отказ от форм ввода паролей и почты внутри лаунчера в пользу официального OAuth2-потока в браузере.
  - Локальный HTTP loopback-сервер (`http://127.0.0.1:25585/callback`) с защитой от CSRF через криптографический `state`.
  - Страница успешной авторизации в браузере на русском языке.
  - Интеграция в графический интерфейс с состояниями ожидания, кнопками повторного открытия ссылки и ручного копирования URL.
  - Консольный вход через `mq accounts add --ely`.
- **Поддержка Minecraft 26.x и хронология манифеста:**
  - Корректная обработка версий схемы `26.x` с извлечением даты релиза из `version_manifest_v2.json`.
  - Динамическое определение требуемой версии Java из `version.json` (`javaVersion.majorVersion`).
  - Проверка совместимости загрузчиков (Fabric, Quilt, NeoForge, Forge).
- **Подсистема модов, шейдеров и ресурспаков:**
  - Генерация файла блокировки зависимостей `.mq-lock.json` с фиксацией версий и хэшей алгоритма SHA-1 / SHA-512.
  - Интеграция движков шейдеров Iris / Sodium и Oculus / Embeddium с автоматической записью `config/iris.properties` и `config/oculus.properties`.
  - Диагностика видеоподсистемы через `glxinfo -B` для безопасного включения шейдеров на совместимых GPU.
  - Атомарное обновление списка активных ресурспаков `resourcePacks:[...]` в `options.txt` без повреждения других настроек игрока.
- **Интерфейс и дизайн-система (Solid Dark Design System):**
  - Новые компоненты `crates/mq-ui/src/components/`: `SearchField` (32px, иконка лупы, быстрая очистка, `Ctrl+F`), `FilterChip` (индикаторы логов с бейджами количества строк), `segmented_switch` (векторный переключатель видов), `Select` (кастомные выпадающие списки), `render_underline_tabs` (акцентные подчеркивания).
  - Устранение 20 ключевых дефектов интерфейса (реестр `docs/UI_DEFECTS.md`).
  - Строгий русский Sentence case во всех экранах и диалогах.
  - Индикатор доступного обновления в боковой панели лаунчера.
  - Диалоговое окно "О программе" и CLI-команда `mq about` с указанием автора (`MrQuster`) и репозитория.

### Изменено
- Окно лаунчера переведено в полностью непрозрачный режим (`with_transparent(false)`), исключающий графические артефакты на Wayland-композиторах (KDE KWin, Hyprland, Sway).
- Улучшена локализованная диагностика падений в `CrashAnalyzer` с рекомендациями по решению проблем OpenGL, GLFW, Iris и устаревших драйверов.
- Обновлены метаданные пакетов `Cargo.toml`, `PKGBUILD`, `mq.desktop` с фиксацией версии `2.1.0` и ссылок на `https://github.com/MrQuster/mq`.

### Исправлено
- Смещение и размытие 1.5px векторных линий на HiDPI экранах (пиксель-снаппинг).
- Падение при запуске окна на тайлинговых оконных менеджерах в момент инициализации с нулевым размером экрана `0x0`.
- Ограничение высоты консоли логов — консоль адаптируется под полный размер свободного пространства окна.

---

## [v2.0.0] — 2026-11-07

Первый стабильный монолитный релиз MQ Launcher.
- Полная русская локализация и автоматическая синхронизация локали `ru_ru` в `options.txt`.
- Офлайн, Microsoft Device Code Flow и базовый вход Ely.by.
- Поддержка Vanilla, Fabric, Quilt, Forge, NeoForge.
- Встроенный каталог Modrinth и CurseForge.
- Управление мирами, скриншотами, резервными копиями и серверами.
- Архитектура на основе Rust, Tokio, egui/eframe.
