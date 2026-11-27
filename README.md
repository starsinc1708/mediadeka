# Mediadeka

Mediadeka — музыкальный плеер для Windows x64. Слушайте музыку с компьютера, домашнего сервера Jellyfin и подключённых сервисов через общую медиатеку, поиск и очередь.

[Выпуски для Windows](https://github.com/starsinc1708/mediadeka/releases) · [Roadmap](ROADMAP.md) · [Как внести вклад](#как-внести-вклад)

![Главная страница Mediadeka с подборками, навигацией и плеером Fluid Glass](assets/mediadeka-home.webp)

Главная страница в гостевом режиме с общедоступными подборками YouTube Music.

## Возможности

- Локальные файлы и подключения Spotify, YouTube Music, Яндекс Музыка, SoundCloud и Jellyfin. Доступность поиска и воспроизведения зависит от источника.
- Общая очередь, плейлисты, история и избранное. Гостевой режим позволяет открыть локальную музыку без входа в сервис.
- Синхронные тексты с подсветкой слов, когда доступна пословная разметка. Нажатие на строку переводит плеер к этой части песни.
- Интерфейс Fluid Glass, полноэкранный плеер, клавиатурное управление, повышенный контраст и уменьшенное движение.

## Как запустить

1. Скачайте ZIP для Windows x64 со [страницы выпусков](https://github.com/starsinc1708/mediadeka/releases), сверьте его SHA-256 по `SHA256SUMS.txt` и распакуйте.
2. Запустите `mediadeka.exe` из распакованной папки. Для знакомства с плеером выберите «Гостевой вход».

В ZIP уже есть `libmpv-2.dll` для воспроизведения. Для входа в SoundCloud через встроенное окно нужен Microsoft Edge WebView2 Runtime.

Первый выпуск рассчитан на Windows x64. [Планы для Linux, macOS, Android и Android TV](ROADMAP.md#выпуски-на-других-платформах)

## Сборка на Windows

Mediadeka написана на Rust с GPUI. Для сборки понадобятся Git, PowerShell 7, Rust `1.100.0` с целью `x86_64-pc-windows-msvc`, Visual Studio Build Tools с компонентами C++ и Windows SDK.

```powershell
rustup toolchain install 1.100.0 --profile minimal --component rustfmt --component clippy
rustup target add --toolchain 1.100.0 x86_64-pc-windows-msvc
rustup override set 1.100.0
Push-Location app2
cargo build --locked --features fluid-glass-default
Pop-Location
```

Для воспроизведения в debug-сборке скачайте тот же libmpv в папку с EXE. Запустите команду из корня репозитория.

```powershell
pwsh -File scripts/Get-Libmpv.ps1 -OutputDirectory app2/target/debug
```

Профиль по умолчанию хранится в `%LOCALAPPDATA%/Mediadeka/app2/`. Для проверки на новом профиле задайте `MEDIADEKA_PROFILE_DIR` в текущем процессе.

## Пакет сборки

```powershell
pwsh -File scripts/Build-Release.ps1
```

Скрипт компилирует оптимизированную сборку с `--locked` и `fluid-glass-default`, скачивает закреплённую сборку libmpv и проверяет её SHA-256. В `dist/` появляются ZIP с `mediadeka.exe` и `libmpv-2.dll` и файл `SHA256SUMS.txt`. Файл `build-info.json` внутри ZIP указывает исходный коммит, версию Rust, хеши EXE и DLL и источник libmpv.

## Как внести вклад

Опишите сценарий в GitHub Issues или приложите его к pull request. Команды проверки запускаются из `app2/`.

```powershell
cargo fmt --all -- --check
cargo clippy --locked --features capture --all-targets -- -D warnings
cargo test --locked --features capture --lib
```
