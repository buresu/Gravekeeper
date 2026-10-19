# Bundled fonts

Embedded into the executable by `src/ui/theme.rs`. Both are subsets, made with fontTools:

```sh
pyftsubset NotoSans-Regular.ttf --unicodes="U+0020-007E,U+00A0-017F,U+2000-206F,U+20AC,U+2190-21FF,U+2212,U+2264,U+2265,U+25CF" \
  --layout-features='kern,liga,calt' --no-hinting --desubroutinize --output-file=NotoSans-Regular.ttf
pyftsubset Phosphor.ttf --unicodes="<code points listed in src/ui/icons.rs>" --no-hinting --output-file=Phosphor.ttf
```

Other scripts (Japanese, Chinese, ...) fall back to the system fonts at runtime.

| File | Source | License |
|---|---|---|
| `NotoSans-Regular.ttf`, `NotoSans-SemiBold.ttf` | Noto Sans | SIL OFL 1.1 (`OFL-NotoSans.txt`) |
| `Phosphor.ttf` | Phosphor Icons, regular weight (via the egui-phosphor crate) | MIT (`LICENSE-Phosphor.txt`) |
