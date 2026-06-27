# Distribuer `mkvhdr10plus` sur Windows

Objectif : produire un dossier autonome que quelqu'un (par ex. un proche) peut
lancer sur Windows **sans rien installer**, par simple glisser-déposer.

Comme `mkvhdr10plus` lie les bibliothèques FFmpeg à la compilation, on ne peut
pas le cross-compiler proprement depuis macOS. La voie fiable est la **CI
GitHub** du dépôt, qui build déjà Windows x64.

## Vue d'ensemble

```
git tag vX.Y.Z && git push origin vX.Y.Z      (1) déclenche la CI
        │
   GitHub Actions (release.yml) build Windows  (2) zip avec mkvhdr10plus.exe + DLL FFmpeg
        │
   télécharger le .zip Windows depuis la Release (3)
        │
   scripts/package_windows_bundle.ps1          (4) ajoute ffmpeg.exe + hdr10plus_tool.exe
        │                                            + .bat glisser-déposer + LISEZ-MOI
   zipper le dossier → envoyer                  (5)
```

## Étape par étape

### 1. Vérifier le code avant de tagger

Le pipeline a évolué (repli `ffmpeg` pour le mux, détection de la cadence) :

```bash
cargo test -p mkvhdr10plus
cargo clippy -p mkvhdr10plus --all-targets -- -D warnings
cargo fmt --all -- --check
```

### 2. Créer un tag de release

`release.yml` se déclenche sur les tags `v*.*.*` :

```bash
# pense à bumper la version dans mkvhdr10plus/Cargo.toml et CHANGELOG.md
git tag v0.3.0
git push origin v0.3.0
```

La CI compile les quatre binaires sur Windows/macOS/Linux et publie une Release.
Le zip Windows (`hdr-analyze-v0.3.0-x86_64-pc-windows-msvc.zip`) contient
`mkvhdr10plus.exe` **et les DLL FFmpeg** dans `bin/` (sans elles l'exe ne
démarrerait pas).

### 3. Récupérer le zip Windows

Depuis la page Releases du dépôt, télécharge l'archive
`...-x86_64-pc-windows-msvc.zip` et décompresse-la. Tu obtiens un dossier `bin/`.

### 4. Assembler le bundle clé-en-main (sur une machine Windows)

Le script ajoute les outils externes (`ffmpeg`, `hdr10plus_tool`, `mkvmerge`),
écrit un lanceur glisser-déposer et un mode d'emploi en français.

`mkvmerge` ne s'auto-télécharge pas proprement (build portable en `.7z`
versionné). Récupère donc une fois **MKVToolNix portable** depuis
<https://mkvtoolnix.download/downloads.html>, décompresse-le, et pointe le
script dessus avec `-MkvmergeDir` :

```powershell
# PowerShell, depuis la racine de l'archive décompressée
.\scripts\package_windows_bundle.ps1 -BinDir .\bin -MkvmergeDir .\mkvtoolnix
```

Résultat : un dossier `mkvhdr10plus-windows\` contenant
`mkvhdr10plus.exe`, les DLL FFmpeg, `ffmpeg.exe`, `hdr10plus_tool.exe`,
`mkvmerge.exe` (+ ses DLL), `Convertir en HDR10+.bat` et `LISEZ-MOI.txt`.

> Pourquoi embarquer mkvmerge ? Le remux d'un flux HEVC **brut** exige un outil
> qui gère le réordonnancement des images (B-frames). `mkvmerge` le fait ;
> `ffmpeg -c copy` échoue (« unknown timestamp »). Le repli ffmpeg de
> `mkvhdr10plus` n'est qu'un secours best-effort — pour une livraison fiable,
> mkvmerge doit être présent (le `.bat` le trouve, car il est dans le dossier).

### 5. Livrer

Zippe `mkvhdr10plus-windows\` et envoie-le. Côté utilisateur : décompresser,
puis glisser un `.mkv` HDR10 sur `Convertir en HDR10+.bat`.

## Et sans GitHub Actions ?

Si la CI n'est pas disponible, il faut un PC Windows avec Rust + LLVM + FFmpeg
(via vcpkg) — voir `.github/workflows/release.yml` pour la liste exacte — puis :

```powershell
cargo build --release -p mkvhdr10plus
```

et copier les DLL FFmpeg de vcpkg à côté de l'exe avant de lancer le script de
packaging avec `-BinDir` pointant sur `target\release`.

## Notes de licence (si distribution publique)

Pour un usage personnel/familial, embarquer `ffmpeg` et `hdr10plus_tool` ne pose
pas de problème pratique. Pour une diffusion publique, pense aux licences :
FFmpeg (LGPL/GPL selon le build), `hdr10plus_tool` (MIT), MKVToolNix (GPL).
Conserve les mentions de licence des outils embarqués dans le dossier livré.
