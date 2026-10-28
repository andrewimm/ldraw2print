---
library: yes
config: default
---
# Handheld torch (exit code 4 path)

Hand-grip detection finds three matching bars; the agent must choose.

## Prompt

Print me a 5:1 minifig torch (the handheld one) in reddish brown for my
figure's hand. If the tool needs me to choose anything, choose sensibly and
explain.

## Good outcome

- Part 3959 (or 86208 with reasoning), `--color 70`.
- Resolves exit 4 with `--connector-candidate 3` (the perpendicular grip
  stub) and explains why.
- Does not use `--smooth` on 3959, or uses it and reports the opened seams.
- Recommends hand-grip calibration coupons (fit unverified).

## Automatic checks

An output file; convert with 3959 and --connector-candidate 3; if --smooth
was used, the report must mention seams.
