#!/bin/sh
# Builds docs/images/demo.gif from the frames made by tests/gui/screenshots.spec.js:
#
#   FERRY_SCREENSHOTS=docs/images npx playwright test tests/gui/screenshots.spec.js
#   scripts/make-demo-gif.sh
#
# Needs ffmpeg. The frames are deleted afterwards.
set -eu
dir="${1:-docs/images}"
ffmpeg -loglevel error -y -framerate 2 -i "$dir/frames/%03d.png" \
  -vf "scale=800:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=96[p];[b][p]paletteuse=dither=none" \
  "$dir/demo.gif"
rm -r "$dir/frames"
echo "$dir/demo.gif"
