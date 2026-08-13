#!/usr/bin/env bash
# Fetches the pinned evaluation corpus into corpus/.
#
# The corpus deliberately spans typesetting pipelines, because the failure modes differ: pdfTeX
# with OT1 Computer Modern breaks ligatures on extraction, while publisher templates use
# different fonts, column rules and heading conventions. Papers are pinned to a specific version
# so extraction output stays comparable across commits.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/corpus"
mkdir -p "$DEST"

# id                       filename                       why it is in the corpus
PAPERS=(
  # Machine learning preprints: the templates most PDFs in this space use.
  "1706.03762v7|transformer.pdf|single column, wide tables, inline math"
  "1512.03385v1|resnet.pdf|two column, full-width figures, numbered equations"
  "1412.6980v9|adam.pdf|algorithm floats, heavy display math"
  "1810.04805v2|bert.pdf|two column, dense tables, footnotes"

  # Other fields, other LaTeX classes. Machine learning uses a narrow set of conference
  # templates, and a converter tuned only on those learns their quirks rather than the general
  # shape of a paper.
  "2607.28606v1|numbertheory.pdf|pure maths: theorem environments, display-heavy"
  "2607.28558v1|optics.pdf|physics: long derivations, subscript-dense notation"
  "2607.28053v1|biology.pdf|life sciences: model tables, mixed figures"
  "2607.28455v1|statistics.pdf|statistics: dense result tables"

  # Older submissions, built by older toolchains.
  "1505.04597v1|unet.pdf|Springer LNCS class, single column"
  "1406.2661v1|gan.pdf|2014 NeurIPS template, algorithm blocks"

  # Publisher templates. arXiv preprints skew towards conference styles; the papers a reader
  # actually hands a converter are as often typeset in a journal class, and each of these has a
  # heading convention, a column geometry or a first-paragraph decoration that the conference
  # templates above never exercise.
  "2109.09450v1|metasurface.pdf|IEEEtran: two column, Roman-numeral headings, \IEEEPARstart drop cap"
  "1806.01973v1|pinsage.pdf|acmart sigconf: two column, ACM heading style, wide result tables"
  "1002.3895v2|topological.pdf|REVTeX (Rev. Mod. Phys.): two column, dense inline maths, long bibliography"
  "1702.05747v2|medimaging.pdf|elsarticle: two column Elsevier, medical prose, many long tables"
  "1409.0575v3|imagenet.pdf|svjour3: two column Springer journal, mixed-width floats"
  "1201.0490v4|sklearn.pdf|JMLR (jmlr2e): single column, wide measure, running heads"
)

for entry in "${PAPERS[@]}"; do
  IFS='|' read -r id name why <<< "$entry"
  out="$DEST/$name"
  if [[ -f "$out" ]]; then
    echo "have    $name"
    continue
  fi
  echo "fetch   $name  ($why)"
  curl -fsSL --retry 3 -A "rustypaper-corpus/0.1" -o "$out" "https://arxiv.org/pdf/$id"
  # arXiv rate-limits; be a good citizen.
  sleep 3
done

echo
echo "corpus in $DEST:"
ls -la "$DEST"/*.pdf
