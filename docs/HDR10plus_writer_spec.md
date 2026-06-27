# Fiche technique — Writer HDR10+ (Profil B)

Spécification pour le module de sortie du fork (génération des métadonnées dynamiques HDR10+ à partir de frames HDR10).

Sources de référence (libres, légitimes) :
- `hdr10plus_tool` (quietvoid) — `hdr10plus/src/metadata.rs` (layout SEI, règles de validation, profils) et `hdr10plus/src/metadata_json.rs` (schéma JSON).
- FFmpeg (LGPL) — `libavutil/hdr_dynamic_metadata.h` (sémantique des champs) et `libswscale/utils.c` (lecture/application, dont le slot réservé du percentile 5 %).

Principe : **le writer produit un JSON**, puis `hdr10plus_tool inject` encode ce JSON en SEI dans le flux HEVC. On n'a donc jamais à écrire le bitstream à la main — il suffit de produire le JSON au bon schéma, aux bonnes unités.

---

## 1. Place dans le pipeline

```
HDR10 MKV
  └─ décode frames (ffmpeg) ──► mesure par frame ──► agrégation scènes ──► metadata.json
                                                                              │
flux HEVC original (extrait, copie bit-à-bit) ──► hdr10plus_tool inject -j metadata.json ──► injected.hevc
                                                                              │
                                                          mkvmerge (remux audio/sous-titres) ──► sortie HDR10+ MKV
```

Aucun ré-encodage : l'injection ajoute les SEI dans le flux existant, qualité intacte.

---

## 2. Mesure par frame (le cœur)

Toutes les valeurs de luminance sont en **lumière linéaire**, normalisées sur `[0, 1]` où **1.0 = 10000 nits** (échelle PQ pleine).

### 2.1 Chaîne de reconstruction par pixel

1. Lire la frame en `yuv420p10le` (10 bits, 3 plans Y, U=Cb, V=Cr).
2. Upsampler la chroma 4:2:0 → pleine résolution (bilinéaire recommandé ; le « plus proche voisin » gonfle les maxima par canal).
3. Normaliser (10-bit, range limité) :
   - `Y'  = (Y_code  - 64) / 876`
   - `Cb  = (Cb_code - 512) / 896`
   - `Cr  = (Cr_code - 512) / 896`
4. YCbCr → R'G'B' (BT.2020 non-constant luminance), R'G'B' restant **codé PQ** :
   - `R' = Y' + 1.4746 · Cr`
   - `G' = Y' − 0.16455 · Cb − 0.57135 · Cr`
   - `B' = Y' + 1.8814 · Cb`
   - clamp chaque canal sur `[0, 1]`.
5. EOTF PQ (ST 2084) → lumière linéaire normalisée `[0, 1]` :
   ```
   m1 = 2610/16384            ; m2 = 2523/4096 * 128
   c1 = 3424/4096             ; c2 = 2413/4096 * 32 ; c3 = 2392/4096 * 32
   p  = E'^(1/m2)
   L  = ( max(p - c1, 0) / (c2 - c3 · p) )^(1/m1)      // L ∈ [0,1], 1.0 = 10000 nits
   ```
   Appliquer à R', G', B' → `R_lin, G_lin, B_lin`.

Fenêtre : `NumberOfWindows = 1` → toutes les mesures portent sur **toute la frame active** (une seule fenêtre couvrant l'image).

### 2.2 Grandeurs à calculer (par frame)

Soit, par pixel, `maxRGB = max(R_lin, G_lin, B_lin)`.

| Champ | Définition | Stockage JSON |
|---|---|---|
| `MaxScl` (R,G,B) | max **par canal** sur tous les pixels : `[max(R_lin), max(G_lin), max(B_lin)]` | entier `round(valeur · 100000)`, ≤ 100000 |
| `AverageRGB` | moyenne de `maxRGB` sur tous les pixels | entier `round(valeur · 100000)`, ≤ 100000 |
| `LuminanceDistributions` | percentiles de la distribution de `maxRGB` (voir 2.3) | entiers `round(valeur · 100000)`, ≤ 100000 |

`MaxScl` est bien un **max par canal** (R, G, B séparément), pas le max du maxRGB.

### 2.3 Distribution — attention au slot réservé

`DistributionIndex` est figé : `[1, 5, 10, 25, 50, 75, 90, 95, 99]` (9 valeurs).
`DistributionValues[i]` = valeur de `maxRGB` au percentile correspondant, **SAUF** :

- L'indice du **« 5 »** (position 2) n'est **pas** le 5e percentile. C'est une valeur « réservée » qui suit en pratique le **pixel le plus lumineux** de la scène (constaté dans le code FFmpeg). Y mettre une quasi-crête robuste : recommandé = **99,98e percentile** de `maxRGB` (à caler, voir §6).

Donc, en pratique :

| position | DistributionIndex | DistributionValue à écrire |
|---|---|---|
| 0 | 1  | 1er percentile de maxRGB |
| 1 | 5  | **réservé ≈ crête** → 99,98e percentile de maxRGB |
| 2 | 10 | 10e percentile |
| 3 | 25 | 25e percentile |
| 4 | 50 | 50e percentile (médiane) |
| 5 | 75 | 75e percentile |
| 6 | 90 | 90e percentile |
| 7 | 95 | 95e percentile |
| 8 | 99 | 99e percentile |

C'est ce qui explique la non-monotonie observée dans les fichiers réels (la valeur du slot « 5 » est élevée alors que le « 10 » est bas).

---

## 3. Découpage en scènes

Réutiliser la détection de coupures existante de l'analyseur (`scene.rs`). Pour chaque frame :

- `SequenceFrameIndex` : index absolu dans le flux, `0 … N−1` (contigu).
- `SceneId` : identifiant de scène, incrémenté à chaque coupure (commence à 0).
- `SceneFrameIndex` : index de la frame **dans sa scène**, repart à 0 à chaque coupure.

**Ordre des frames** : `hdr10plus_tool` réordonne par défaut en **ordre d'affichage** (POC). L'injecteur attend donc les entrées `SceneInfo` dans l'ordre d'affichage, une par frame affichée. L'analyseur décode en ordre d'affichage via ffmpeg → cohérent. (Le désalignement rencontré pendant la calibration venait de la comparaison seek↔métadonnées, pas de la génération.)

---

## 4. Structure JSON complète (attendue par `hdr10plus_tool inject`)

Tous les champs ci-dessous sont **obligatoires** au parsing (sauf `BezierCurveData` qui dépend du profil).

```json
{
  "JSONInfo": { "HDR10plusProfile": "B", "Version": "1.0" },
  "SceneInfo": [
    {
      "BezierCurveData": {
        "Anchors": [102, 205, 307, 410, 512, 614, 717, 819, 922],
        "KneePointX": 0,
        "KneePointY": 0
      },
      "LuminanceParameters": {
        "AverageRGB": 247,
        "LuminanceDistributions": {
          "DistributionIndex": [1, 5, 10, 25, 50, 75, 90, 95, 99],
          "DistributionValues": [14, 7319, 91, 42, 80, 192, 630, 1228, 7289]
        },
        "MaxScl": [10568, 10238, 34997]
      },
      "NumberOfWindows": 1,
      "TargetedSystemDisplayMaximumLuminance": 1000,
      "SceneFrameIndex": 0,
      "SceneId": 0,
      "SequenceFrameIndex": 0
    }
    // … une entrée par frame …
  ],
  "SceneInfoSummary": {
    "SceneFirstFrameIndex": [0, 75, 417, ...],
    "SceneFrameNumbers": [75, 342, ...]
  },
  "ToolInfo": { "Tool": "mkvhdr10plus", "Version": "0.1.0" }
}
```

- `SceneInfoSummary.SceneFirstFrameIndex` : liste des `SequenceFrameIndex` où `SceneFrameIndex == 0` (débuts de scène).
- `SceneInfoSummary.SceneFrameNumbers` : longueur de chaque scène (différence entre débuts consécutifs ; la dernière va jusqu'à la fin).

---

## 5. Profil B — Bezier identité + valeurs fixes

On ne calcule **aucune courbe**. On émet une Bezier identité (rampe linéaire) sur toutes les frames :

- `Anchors` = `[102, 205, 307, 410, 512, 614, 717, 819, 922]` (9 points, rampe régulière ≈ identité).
- `KneePointX = 0`, `KneePointY = 0`.
- `TargetedSystemDisplayMaximumLuminance` : doit être **non nul** pour le profil B. Valeur raisonnable : 1000 (le fichier de référence disséqué utilisait 400 — valeur à choisir selon l'intention ; sans tone-mapping réel, l'impact est limité mais le champ doit exister).
- `NumberOfWindows = 1`.

Ces valeurs sont reprises telles quelles d'un fichier HDR10+ commercial qui passe la validation.

---

## 6. Règles de validation (tirées du code — à respecter sous peine de rejet)

`hdr10plus_tool` rejette le JSON si :

- une valeur de `MaxScl`, `AverageRGB` ou `DistributionValues` dépasse **100000** ;
- `DistributionIndex` ≠ `[1,5,10,25,50,75,90,95,99]` (9) ou `[1,5,10,25,50,75,90,95,98,99]` (10) ;
- `num_distribution_maxrgb_percentiles` ∉ {9, 10} ;
- Profil B avec `TargetedSystemDisplayMaximumLuminance == 0` (interdit) ;
- Profil A avec `TargetedSystemDisplayMaximumLuminance != 0` (interdit) ;
- `TargetedSystemDisplayMaximumLuminance` > 100000.

→ Clamp systématiquement toutes les valeurs de luminance à `[0, 100000]` avant écriture.

---

## 7. Chaîne d'injection / mux (après génération du JSON)

```bash
# 1. extraire le flux HEVC original (copie, sans ré-encodage)
ffmpeg -i input.mkv -map 0:v:0 -c copy -bsf:v hevc_mp4toannexb -f hevc original.hevc

# 2. injecter les métadonnées HDR10+ générées
hdr10plus_tool inject -i original.hevc -j metadata.json -o injected.hevc

# 3. remux avec l'audio et les sous-titres d'origine
mkvmerge -o output_hdr10plus.mkv injected.hevc --no-video input.mkv

# 4. vérifier
hdr10plus_tool --verify extract output_hdr10plus.mkv
```

---

## 8. Stratégie de validation / calage final

Une fois le writer codé :

1. Prendre un vrai fichier HDR10+ de référence, extraire son `metadata.json` (`hdr10plus_tool extract`).
2. Sur **une frame dont l'alignement est garanti** (extraction frame-exacte `select=eq(n,X)` sans seek), comparer `MaxScl`, `AverageRGB` et la distribution générés à ceux de la référence.
3. Caler les points encore ouverts :
   - **Méthode d'interpolation des percentiles** (linéaire entre échantillons vs « nearest-rank ») — peut décaler les valeurs de quelques %.
   - **Définition exacte du slot « 5 % » réservé** : confirmer 99,98e percentile vs max strict.
   - **Upsampling chroma** : vérifier que le bilinéaire ne gonfle pas `MaxScl`.
4. Tolérance cible : structure correcte (canal dominant) + valeurs à quelques % de la référence. Un expert verra une dérive de structure, pas une erreur de 2 % sur un percentile.

---

## 9. Récapitulatif des inconnues restantes

| Élément | Statut |
|---|---|
| Domaine (linéaire vs PQ) | **Résolu** : linéaire (confirmé FFmpeg) |
| Échelle des valeurs | **Résolu** : ×100000, 1.0 = 10000 nits |
| Sémantique MaxScl / AverageRGB | **Résolu** : max par canal / moyenne du maxRGB linéaire |
| Slot « 5 % » de distribution | **Résolu** : valeur réservée ≈ crête (≈ 99,98e pct) |
| Bezier | **Contourné** : identité fixe (profil B) |
| Schéma JSON + validation | **Résolu** : depuis le code |
| Interpolation exacte des percentiles | À caler empiriquement (§8) |
| Convention de fenêtre/crop | À caler (single window plein cadre = point de départ) |
