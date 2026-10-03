# Pictures

An Ice Commander viewer plugin that shows pictures, including the JPEG preview
embedded in a camera raw file. It registers one viewer, `imgviewer`, and one
translated phrase in fifteen languages. No filesystem, no panel, no toolbar.

The plugin refuses to load in the terminal frontend.

## What it shows

Drawn by the frontend as they are:

`.png .jpg .jpeg .gif .bmp .webp .ico .svg .avif .jxl .tif .tiff`

Camera raw, shown through the embedded JPEG:

`.nef .cr2 .cr3 .arw .dng .raf .orf .rw2 .pef .srw .x3f`

Matching is by extension only: the lowercased name ends with one of them. The
two lists must not overlap, which a test checks, because `is_raw` decides which
path a file takes.

## How it works

- **Ordinary picture.** The plugin reads and decodes nothing. Its document names
  the file as `file:<name>`. The host reads it through the filesystem the
  window was opened on, and the frontend draws it: GTK through gdk-pixbuf, the
  web frontend with an `<img>`.
- **Camera raw.** The plugin reads the whole file through the host, finds a
  JPEG through its TIFF/EXIF tags (`kamadak-exif`), and serves it as
  `part:photo/<index>`. A request for an index the window is no longer on gets
  nothing. There is no raw decoder. If no JPEG is found, the window shows
  "There is no photograph inside this file." (`imgviewer.no_photograph`).
- **The window.** The picture is fitted and zoomable. Below it are ◀ and ▶
  buttons (also `Left` and `Right`), then the file name, its size and its
  position, such as `3 / 12`. A button is insensitive at either end.
- **Walking the folder.** The source's root is listed once, when the window
  opens. Entries that are not directories and match either list are sorted by
  name, ignoring case, and the window starts on the opened file. If the
  listing fails or does not contain that file, the window holds only that
  file.
- **Size.** The size comes from a seek to the end of the file, repeated on
  every move.
- **Several windows.** State is kept per host instance number, so two open
  windows never answer for each other.

## Known limitations

- **Folder walking works on the local disk only.** The GTK and web hosts root
  the viewer's source at the file's folder only for the local filesystem. For
  any other filesystem (a server, an archive, a plugin filesystem), the host
  first copies the single file into a scratch folder, so the window holds that
  one file and both buttons stay insensitive.
- **Raw containers.** `kamadak-exif` 0.5 accepts a raw file only if it starts
  with a standard TIFF header (`II*\0` or `MM\0*`). CR3 (ISO BMFF), RAF, ORF,
  RW2 and X3F have their own headers, so they always show the "no photograph"
  text.
- **Raw previews.** Only the main IFD chain is searched, not SubIFDs, so a
  preview stored only in a SubIFD is not found. Only the first strip or tile
  offset of each IFD is tried. That offset must start with `FF D8 FF` and have
  a byte count.
- **Re-reading raw files.** A raw file is read whole into memory every time
  the window moves to it. Nothing is cached.
- **Decoding depends on the frontend.** GTK draws `.webp`, `.avif`, `.jxl` and
  `.svg` only where a gdk-pixbuf loader for them is installed. The web
  frontend draws what the browser's `<img>` supports.

## Building

```sh
./build.sh          # release build, libraries collected into bin/
./test.sh           # cargo test --workspace
./deploy-local.sh   # copy bin/ libraries into the local plugin folder
```

`deploy-local.sh` deploys to `$IC_PLUGIN_DIR` if set. Otherwise it uses the
platform's `ice-commander/plugins` data folder. Then enable the plugin in
**Settings → Plugins** and restart.

## Licence

MIT or Apache-2.0, at your option. Contributions are accepted under the
Developer Certificate of Origin in `DCO`. Sign off with `git commit -s`.
