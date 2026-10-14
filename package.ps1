# Собирает самодостаточный релиз VoiceAI: exe + CUDA-библиотеки (cuBLAS) + VC++ runtime.
# После сборки кладёт всё в dist/VoiceAI и (по умолчанию) упаковывает в zip.
#
# Пример:
#   pwsh .\package.ps1
#   pwsh .\package.ps1 -SkipBuild
#   pwsh .\package.ps1 -CudaBin "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4\bin\x64"

param(
    [string]$CudaBin = "",
    [switch]$SkipBuild,
    [switch]$SkipZip
)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot

function Fail($msg) {
    Write-Host "ОШИБКА: $msg" -ForegroundColor Red
    exit 1
}

# --- 1. Сборка ---
$exeName = "whisper_flow_rust.exe"
$exePath = Join-Path $root "target\release\$exeName"

if (-not $SkipBuild) {
    Write-Host "==> cargo build --release" -ForegroundColor Cyan
    & cargo build --release --manifest-path (Join-Path $root "Cargo.toml")
    if ($LASTEXITCODE -ne 0) { Fail "cargo build завершился с ошибкой" }
}

if (-not (Test-Path -LiteralPath $exePath)) {
    Fail "Не найден собранный exe: $exePath (запустите без -SkipBuild)"
}

# --- 2. Поиск CUDA-библиотек ---
$requiredCuda = @("cublas64_13.dll", "cublasLt64_13.dll")

$cudaCandidates = @()
if ($CudaBin) { $cudaCandidates += $CudaBin }
if ($env:CUDA_PATH) { $cudaCandidates += (Join-Path $env:CUDA_PATH "bin\x64") }
$cudaCandidates += (Get-ChildItem "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA" -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | ForEach-Object { Join-Path $_.FullName "bin\x64" })

$cudaDir = $null
foreach ($cand in $cudaCandidates) {
    if (-not $cand) { continue }
    if (Test-Path -LiteralPath (Join-Path $cand "cublas64_13.dll")) { $cudaDir = $cand; break }
}
if (-not $cudaDir) {
    Fail "Не найдены CUDA-библиотеки (cublas64_13.dll). Укажите -CudaBin <путь\к\bin\x64>"
}

# --- 3. Поиск VC++ runtime ---
$vcNames = @("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll")
$vcCandidates = @(
    (Get-ChildItem "C:\Program Files\Microsoft Visual Studio" -Recurse -Directory `
        -Filter "Microsoft.VC143.CRT" -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } | Select-Object -ExpandProperty FullName)
)
$vcCandidates += "C:\Windows\System32"

$vcDir = $null
foreach ($cand in $vcCandidates) {
    $all = $true
    foreach ($n in $vcNames) { if (-not (Test-Path -LiteralPath (Join-Path $cand $n))) { $all = $false; break } }
    if ($all) { $vcDir = $cand; break }
}
if (-not $vcDir) { Fail "Не найдены DLL VC++ runtime ($($vcNames -join ', '))" }

# --- 4. Версия ---
$cargoToml = Get-Content (Join-Path $root "Cargo.toml") -Raw
$version = "0.0.0"
if ($cargoToml -match '(?m)^\s*version\s*=\s*"([^"]+)"') { $version = $Matches[1] }

# --- 5. Сборка папки dist/VoiceAI ---
$outDir = Join-Path $root "dist\VoiceAI"
if (Test-Path -LiteralPath $outDir) { Remove-Item -LiteralPath $outDir -Recurse -Force }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

Copy-Item -LiteralPath $exePath -Destination $outDir -Force
foreach ($n in $requiredCuda) { Copy-Item -LiteralPath (Join-Path $cudaDir $n) -Destination $outDir -Force }
foreach ($n in $vcNames) { Copy-Item -LiteralPath (Join-Path $vcDir $n) -Destination $outDir -Force }

$readme = @"
VoiceAI (Whisper Flow) — портативная сборка для Windows x64
===========================================================

Запуск:
  1. Запустите whisper_flow_rust.exe
  2. Модель Whisper (~1.6 ГБ) скачается автоматически при первом запуске
     (нужен интернет; прогресс видно во вкладке «Настройки»).
  3. Зажмите F9 — говорите, отпустите F9 — текст вставится в активное окно.
  4. F12 — выход из приложения.

Что уже вложено в комплект (устанавливать CUDA и VC++ НЕ нужно):
  - cublas64_13.dll, cublasLt64_13.dll — библиотеки CUDA cuBLAS (для работы на GPU)
  - msvcp140.dll, vcruntime140.dll, vcruntime140_1.dll — Visual C++ Redistributable

Требования:
  - Windows 10/11 x64
  - NVIDIA GPU (для ускорения). Без NVIDIA GPU приложение запустится,
    но распознавание может работать медленно или не работать.

Настройки (ключи, слова-заместители, путь к модели, хоткеи) — во вкладке «Настройки».
Файлы history.jsonl и .env создаются рядом с exe.

Все CUDA-библиотеки распространяются по лицензии CUDA Toolkit EULA (NVIDIA).
"@
Set-Content -LiteralPath (Join-Path $outDir "ПРОЧТИ_МЕНЯ.txt") -Value $readme -Encoding UTF8

# --- 6. Итог ---
$totalSize = (Get-ChildItem -LiteralPath $outDir -File | Measure-Object Length -Sum).Sum
Write-Host ("==> Готово: {0}" -f $outDir) -ForegroundColor Green
Write-Host ("    Файлов: {0}, суммарно: {1:N1} МБ" -f (Get-ChildItem -LiteralPath $outDir -File).Count, ($totalSize / 1MB))

if (-not $SkipZip) {
    $zipPath = Join-Path $root "dist\VoiceAI-$version-win64.zip"
    if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
    Write-Host "==> Упаковка zip (может занять несколько минут, файлы большие)..." -ForegroundColor Cyan
    Compress-Archive -Path (Join-Path $outDir "*") -DestinationPath $zipPath -CompressionLevel Optimal
    $zipSize = (Get-Item -LiteralPath $zipPath).Length
    Write-Host ("==> Архив: {0} ({1:N1} МБ)" -f $zipPath, ($zipSize / 1MB)) -ForegroundColor Green
}
