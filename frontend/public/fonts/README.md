# Fonts

Guvercin ships with **Hanken Grotesk** (`HankenGrotesk-latin.woff2`, `HankenGrotesk-latin-ext.woff2`) as its default typeface — SIL Open Font License, free to use and redistribute.

## Using a different font locally

If you own a license for another typeface and want to use it on your own machine, drop the `.woff2`/`.woff` files here and add matching `@font-face` rules in `frontend/src/fonts.css`. This directory is covered by `.gitignore` for any file that isn't `HankenGrotesk-*.woff2`, so a licensed font you add here won't be committed or redistributed with the repo.
