<#
.SYNOPSIS
    Assemble a self-contained Windows folder for mkvhdr10plus that a
    non-technical user can run by drag-and-drop.

.DESCRIPTION
    Takes the `bin/` folder from a Windows release archive (which contains
    mkvhdr10plus.exe plus the bundled FFmpeg runtime DLLs) and produces a
    ready-to-ship folder that also includes the external CLI tools the
    end-to-end chain needs:

        - ffmpeg.exe        (HEVC extraction + remux fallback)
        - hdr10plus_tool.exe (HDR10+ SEI injection)

    mkvmerge is intentionally NOT required: when it is absent, mkvhdr10plus
    remuxes with ffmpeg instead.

    The script downloads ffmpeg and hdr10plus_tool from their official release
    pages, then writes a French README and a drag-and-drop launcher.

.PARAMETER BinDir
    Folder containing mkvhdr10plus.exe and the FFmpeg *.dll files (the `bin/`
    directory extracted from hdr-analyze-*-x86_64-pc-windows-msvc.zip).

.PARAMETER OutDir
    Destination folder for the assembled bundle.

.PARAMETER MkvmergeDir
    Folder of an extracted MKVToolNix *portable* build. mkvmerge.exe and its
    DLLs are copied into the bundle so the remux step is reliable (ffmpeg copy
    of raw HEVC is not, because of frame reordering). Strongly recommended.
    Download the portable .7z from https://mkvtoolnix.download/downloads.html

.PARAMETER SkipDownloads
    Do not download ffmpeg / hdr10plus_tool (use if you already placed them in
    OutDir manually).

.EXAMPLE
    .\package_windows_bundle.ps1 -BinDir .\hdr-analyze-v0.3.0-x86_64-pc-windows-msvc\bin -MkvmergeDir .\mkvtoolnix
#>

[CmdletBinding()]
param(
    [string]$BinDir = ".\bin",
    [string]$OutDir = ".\mkvhdr10plus-windows",
    [string]$MkvmergeDir = "",
    [string]$FFmpegUrl = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
    [string]$Hdr10PlusToolUrl = "https://github.com/quietvoid/hdr10plus_tool/releases/latest/download/hdr10plus_tool-x86_64-pc-windows-msvc.zip",
    [switch]$SkipDownloads
)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Write-Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

# --- Validate inputs -------------------------------------------------------
$mainExe = Join-Path $BinDir "mkvhdr10plus.exe"
if (-not (Test-Path $mainExe)) {
    throw "mkvhdr10plus.exe not found in '$BinDir'. Point -BinDir at the bin/ folder from the Windows release archive."
}

Write-Step "Creating bundle folder: $OutDir"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# --- Copy the binary + its FFmpeg runtime DLLs -----------------------------
Write-Step "Copying mkvhdr10plus.exe and FFmpeg runtime DLLs"
Copy-Item $mainExe -Destination $OutDir -Force
$dlls = Get-ChildItem -Path $BinDir -Filter *.dll -ErrorAction SilentlyContinue
if ($dlls) {
    $dlls | Copy-Item -Destination $OutDir -Force
} else {
    Write-Warning "No *.dll found next to mkvhdr10plus.exe. If the exe fails to start, the FFmpeg runtime DLLs are missing from the release archive."
}

# --- Helper: download a zip and extract one named .exe into OutDir ----------
function Get-ToolExe {
    param([string]$Url, [string]$ExeName)

    $dest = Join-Path $OutDir $ExeName
    if (Test-Path $dest) {
        Write-Host "    $ExeName already present, skipping download."
        return
    }

    $tmpZip = Join-Path $env:TEMP ("mkvh10p_" + [System.IO.Path]::GetRandomFileName() + ".zip")
    $tmpDir = Join-Path $env:TEMP ("mkvh10p_" + [System.IO.Path]::GetRandomFileName())
    try {
        Write-Step "Downloading $ExeName"
        Invoke-WebRequest -Uri $Url -OutFile $tmpZip -UseBasicParsing
        New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null
        Expand-Archive -Path $tmpZip -DestinationPath $tmpDir -Force
        $found = Get-ChildItem -Path $tmpDir -Recurse -Filter $ExeName | Select-Object -First 1
        if (-not $found) {
            throw "Could not find $ExeName inside the downloaded archive ($Url)."
        }
        Copy-Item $found.FullName -Destination $dest -Force
        Write-Host "    -> $ExeName ready."
    }
    finally {
        Remove-Item $tmpZip -ErrorAction SilentlyContinue
        Remove-Item $tmpDir -Recurse -ErrorAction SilentlyContinue
    }
}

if (-not $SkipDownloads) {
    Get-ToolExe -Url $FFmpegUrl -ExeName "ffmpeg.exe"
    # ffprobe is handy for diagnostics; ignore if absent in the chosen build.
    try { Get-ToolExe -Url $FFmpegUrl -ExeName "ffprobe.exe" } catch { Write-Warning $_ }
    Get-ToolExe -Url $Hdr10PlusToolUrl -ExeName "hdr10plus_tool.exe"
} else {
    Write-Step "Skipping downloads (--SkipDownloads). Make sure ffmpeg.exe and hdr10plus_tool.exe are in $OutDir."
}

# --- mkvmerge (recommended; reliable remux of reordered HEVC) ---------------
if ($MkvmergeDir) {
    $mkvExe = Get-ChildItem -Path $MkvmergeDir -Recurse -Filter mkvmerge.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $mkvExe) {
        throw "mkvmerge.exe not found under '$MkvmergeDir'. Point -MkvmergeDir at an extracted MKVToolNix portable folder."
    }
    Write-Step "Bundling mkvmerge and its DLLs"
    $srcDir = $mkvExe.Directory.FullName
    Copy-Item $mkvExe.FullName -Destination $OutDir -Force
    Get-ChildItem -Path $srcDir -Filter *.dll -ErrorAction SilentlyContinue |
        Copy-Item -Destination $OutDir -Force
} else {
    Write-Warning "No -MkvmergeDir given: mkvmerge will NOT be bundled. The remux will fall back to ffmpeg, which is unreliable for HEVC. Download MKVToolNix portable and re-run with -MkvmergeDir."
}

# --- Drag-and-drop launcher ------------------------------------------------
Write-Step "Writing drag-and-drop launcher"
$bat = @'
@echo off
setlocal
rem Make the bundled tools (ffmpeg, hdr10plus_tool) take priority on PATH.
set "PATH=%~dp0;%PATH%"

if "%~1"=="" (
  echo.
  echo   Glissez-deposez un fichier .mkv HDR10 sur ce fichier pour le convertir.
  echo.
  pause
  exit /b 1
)

echo Conversion HDR10 -^> HDR10+ : "%~1"
"%~dp0mkvhdr10plus.exe" "%~1" --verify
echo.
echo Termine. Le fichier .HDR10plus.mkv est a cote de la source.
pause
'@
Set-Content -Path (Join-Path $OutDir "Convertir en HDR10+.bat") -Value $bat -Encoding ASCII

# --- French README ---------------------------------------------------------
Write-Step "Writing README"
$readme = @'
mkvhdr10plus — convertisseur HDR10 vers HDR10+
==============================================

Ce dossier est autonome : tout ce qu'il faut est deja dedans, rien a installer.

UTILISATION SIMPLE (glisser-deposer)
------------------------------------
1. Glissez votre fichier .mkv HDR10 sur "Convertir en HDR10+.bat".
2. Patientez (la mesure image par image peut etre longue sur un film 4K complet).
3. Un nouveau fichier "<nom>.HDR10plus.mkv" apparait a cote de l'original.
   Le fichier d'origine n'est jamais modifie ni supprime.

UTILISATION EN LIGNE DE COMMANDE (optionnel)
--------------------------------------------
Ouvrez une invite de commande dans ce dossier puis :

    mkvhdr10plus.exe "film.mkv" --verify

Options utiles :
    --json-only            genere seulement les metadonnees (aucun reassemblage)
    --downscale 2          analyse en demi-resolution (plus rapide)
    --sample-rate 2        analyse une image sur deux (plus rapide)
    --target-nits 1000     luminance cible des metadonnees (defaut 1000)
    -o "sortie.mkv"        chemin de sortie

LECTURE SUR TELEVISEUR
----------------------
Le HDR10+ est pris en charge par les TV Samsung (et plusieurs Panasonic,
Hisense, TCL, Philips). Les TV LG ne lisent PAS le HDR10+ (elles font du Dolby
Vision) : le fichier y sera lu en HDR10 classique.
Methode la plus fiable : copier le .mkv sur une cle USB et le lire avec le
lecteur multimedia integre de la TV.

REMARQUES
---------
- Aucun ré-encodage : la qualite de l'image est preservee a l'identique, seules
  des metadonnees HDR10+ sont ajoutees.
- Outils embarques : ffmpeg, hdr10plus_tool et mkvmerge (dans ce meme dossier).
'@
Set-Content -Path (Join-Path $OutDir "LISEZ-MOI.txt") -Value $readme -Encoding UTF8

# --- Summary ---------------------------------------------------------------
Write-Host ""
Write-Step "Bundle ready: $OutDir"
Get-ChildItem $OutDir | Select-Object Name, Length | Format-Table -AutoSize
Write-Host "Zip this folder and send it to the end user." -ForegroundColor Green
