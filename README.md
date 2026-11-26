<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/m3/hero-ru-dark.svg">
  <img src="docs/assets/m3/hero-ru-light.svg" width="100%" alt="MQ Launcher — быстрый модульный лаунчер Minecraft на Rust">
</picture>

<br/><br/>

<a href="https://github.com/MrQuster/MQ-Launcher/releases/latest"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/m3/btn-download-ru-dark.svg"><img src="docs/assets/m3/btn-download-ru-light.svg" width="26.2%" alt="Скачать v2.8.4"></picture></a>&nbsp;&nbsp;<a href="#установка"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/m3/btn-install-ru-dark.svg"><img src="docs/assets/m3/btn-install-ru-light.svg" width="21.0%" alt="Установка"></picture></a>&nbsp;&nbsp;<a href="#скриншоты"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/m3/btn-screens-ru-dark.svg"><img src="docs/assets/m3/btn-screens-ru-light.svg" width="21.9%" alt="Скриншоты"></picture></a>&nbsp;&nbsp;<a href="CHANGELOG.md"><picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/m3/btn-changelog-ru-dark.svg"><img src="docs/assets/m3/btn-changelog-ru-light.svg" width="21.9%" alt="Изменения"></picture></a>

<br/><br/>

[![Release](https://img.shields.io/github/v/release/MrQuster/MQ-Launcher?style=for-the-badge&color=425E91&labelColor=1E2025&label=Release)](https://github.com/MrQuster/MQ-Launcher/releases)
[![Downloads](https://img.shields.io/github/downloads/MrQuster/MQ-Launcher/total?style=for-the-badge&color=2B614F&labelColor=1E2025&label=Downloads)](https://github.com/MrQuster/MQ-Launcher/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/MrQuster/MQ-Launcher/ci.yml?style=for-the-badge&color=3E4759&labelColor=1E2025&label=CI)](https://github.com/MrQuster/MQ-Launcher/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-3E4759?style=for-the-badge&labelColor=1E2025)](#установка)
[![Rust](https://img.shields.io/badge/rust-1.80%2B-3E4759?style=for-the-badge&labelColor=1E2025)](#сборка-из-исходников)

**Русский** · [English](README.en.md)

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/m3/wave-dark.svg">
  <img src="docs/assets/m3/wave-light.svg" width="100%" alt="">
</picture>

</div>

## Возможности

<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/m3/features-ru-dark.svg">
  <img src="docs/assets/m3/features-ru-light.svg" width="100%" alt="Ключевые возможности MQ Launcher">
</picture>

</div>

- **Изоляция сборок** — отдельные каталоги `mods/`, `resourcepacks/`, `shaderpacks/`, `config/` и `saves/` для каждого профиля, поддержка версий от `1.7.10` до `26.1` (`Релизы`, `Снапшоты`, `Beta`, `Alpha`), загрузчиков `Vanilla`, `Fabric`, `Quilt`, `Forge` и `NeoForge`, бэкапы миров и анализ крашей.
- **Каталог Modrinth и CurseForge** — поиск модов, ресурс-паков, шейдеров и модпаков, пакетная очередь с автоматической дедупликацией зависимостей, импорт `.mrpack`/`.zip`, проверка обновлений и управление уже установленными файлами прямо в инстансе.
- **Аккаунты без лишних настроек** — Microsoft (OAuth2 Device Code Flow) и Ely.by (OAuth2 + PKCE S256 с `authlib-injector`) работают сразу из коробки, плюс локальный офлайн-режим со скином.

---

## Скриншоты

<div align="center">

| Инстансы | Моды инстанса |
| :---: | :---: |
| ![Экран инстансов](docs/assets/screenshots/01-instances.png) | ![Моды инстанса](docs/assets/screenshots/02-detail-mods.png) |

| Каталог модов и сборки | Оформление (тёмная тема) |
| :---: | :---: |
| ![Каталог Modrinth и CurseForge](docs/assets/screenshots/04-catalog.png) | ![Настройки оформления](docs/assets/screenshots/06-settings-matte.png) |

| Аккаунты | Светлая тема |
| :---: | :---: |
| ![Менеджер аккаунтов](docs/assets/screenshots/05-accounts.png) | ![Светлая тема Light Clean](docs/assets/screenshots/07-settings-light.png) |

</div>

<details>
<summary>Ещё скриншот: управление шейдерами</summary>

<div align="center">

![Управление шейдерами в инстансе](docs/assets/screenshots/03-shaders.png)

</div>

</details>

---

## Установка

Готовые сборки и контрольные суммы (`SHA256SUMS`) публикуются на странице [GitHub Releases](https://github.com/MrQuster/MQ-Launcher/releases/latest).

### Windows 10 / 11 (x64)

Быстрая установка одной командой в PowerShell:

```powershell
irm https://github.com/MrQuster/MQ-Launcher/releases/latest/download/install.ps1 | iex
```

Или выберите готовый артефакт в релизе:
- **Установщик:** `MQ-Setup-2.8.4-x64.exe`
- **Портативная версия:** `mq-windows-x86_64-portable.zip` (также доступны манифесты `packaging/winget` и `packaging/scoop`)

> [!TIP]
> Файл `portable.flag` рядом с `mq.exe` переводит лаунчер в полностью портативный режим: все инстансы, настройки и кэш хранятся в папке рядом с программой.

### Linux (x86_64)

Быстрая установка одной командой (загрузка релиза, проверка SHA-256 и интеграция с меню приложений):

```bash
curl -fsSL https://github.com/MrQuster/MQ-Launcher/releases/latest/download/install.sh | sh
```

Также доступны архив `mq-linux-x86_64.tar.gz`, standalone-бинарник `mq-linux-x86_64` и рецепт для Arch Linux в [`packaging/PKGBUILD`](packaging/PKGBUILD).

---

## Безопасность и приватность

<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/m3/security-ru-dark.svg">
  <img src="docs/assets/m3/security-ru-light.svg" width="100%" alt="Безопасность и приватность MQ Launcher">
</picture>

</div>

Подробная документация: [SECURITY.md](docs/SECURITY.md) · [PRIVACY.md](docs/PRIVACY.md) · [THREAT_MODEL.md](docs/THREAT_MODEL.md) · [FAQ.md](docs/FAQ.md)

---

## Сборка из исходников

<details>
<summary>Инструкция для разработчиков (Rust 1.80+)</summary>

Требуется инструментарий Rust (`1.80+`, зафиксирован в [`rust-toolchain.toml`](rust-toolchain.toml)).

```bash
# 1. Клонирование репозитория
git clone https://github.com/MrQuster/MQ-Launcher.git
cd MQ-Launcher

# 2. Запуск GUI из исходников
cargo run -p mq-cli -- ui

# 3. Релизная сборка
cargo build --release -p mq-cli

# 4. Проверка форматирования, Clippy и тестов
./scripts/verify.sh
```

Исполняемый файл собирается в `target/release/mq` (`target\release\mq.exe` на Windows). Зависимости сборки и правила контрибьюта описаны в [CONTRIBUTING.md](CONTRIBUTING.md).

</details>

---

<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/m3/wave-dark.svg">
  <img src="docs/assets/m3/wave-light.svg" width="100%" alt="">
</picture>

</div>

MQ использует открытые спецификации и инструменты сообщества: [Modrinth](https://modrinth.com/), [Ely.by](https://ely.by/), [authlib-injector](https://github.com/yushijinhun/authlib-injector), [Fabric](https://fabricmc.net/), [Quilt](https://quiltmc.org/) и [NeoForge](https://neoforged.net/). Лицензии библиотек перечислены в [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).

Автор: [MrQuster](https://github.com/MrQuster). MQ Launcher является независимым проектом с открытым исходным кодом и не связан с Mojang Studios или Microsoft Corporation. Для игры на серверах с `online-mode=true` требуется лицензионный аккаунт Minecraft.
