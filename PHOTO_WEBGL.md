# Optional Windows WebGL photo map (local prototype)

The regular PicSee build and the **照片地圖** (Photo map) panel still provide the native coordinate plot. This experimental Windows build adds **照片地圖 → 開啟互動地圖**. It needs Microsoft Edge WebView2 already present. No installer, server, network tiles or account are required to use the offline grid. The grid is generated in the view and is not a street map.

```powershell
cargo build --locked --features photo-webgl --target-dir target/photo-map-prototype
./prepare-photo-map.ps1 -ExeDirectory ./target/photo-map-prototype/debug
./target/photo-map-prototype/debug/mypicsee.exe
```

- Open a JPEG, choose **讀取這張** or **掃描此資料夾** in **照片地圖**, then open the map. Opening the map does not scan; opening normal PicSee does not initialize WebView2.
- Gold marks the current photo when it has a result in the active scan. Clicking a map point selects the photo; shared or nearby pins filter the native photo list so you can select one, and **顯示全部** resets the filter. The photos, paths and SQLite stay native. Positions beyond 85° latitude are clamped for plotting only, while the original coordinates remain visible in the list.
- Closing the map keeps the active photo. If WebView2 is missing or startup fails, or the local assets are missing or mismatched, the native GPS plot remains available.
- The Windows WebView uses a fixed local resource protocol, not a listening HTTP server. The WebView cannot connect to the Internet: CSP allows images only from the local origin and generated data URLs, navigation and popups are restricted, and selection messages are bounded numeric IDs for the current scan. No filename, image bytes, cache contents or folder path are sent to the view.

## Local map engine and licensing

The preparation script copies exactly two pinned files, `easymap.js` and `dg_webgl_point.js`, from the local `3wa_webgl_map` checkout at `bef84f8c2ff5ff237e2f918ef2bba6b00667ef53`. It leaves that checkout untouched and places the verified files in `picsee-map-assets/` beside this particular executable. The app verifies the same SHA-256 fingerprints. Don't copy those private engine files into the public PicSee repository or redistribute them unless that project has separately granted permission. They are not part of PicSee's MIT license. The optional WebView2 build is a local test build, not a prepared release package.

## Quick check

Use three JPEGs with valid GPS (two with identical coordinates) and one with no GPS. Verify both directions, including selecting either co-located photo from a cluster, selecting the no-GPS photo to clear the active marker, closing the map without changing selection, and reopening. Move/resize the viewer and confirm the map stays within its panel. For a fallback check, keep the prototype assets absent initially. Confirm normal viewing never opens a map before the button is pressed.

## Optional OpenStreetMap raster basemap

After opening WebGL, check **使用 OpenStreetMap（連線）** to overlay real OpenStreetMap streets and place names. This is off by default; closing the map or opening a different photo folder turns it off. OpenStreetMap receives your IP and the visible tile area after each view change. It does not receive photo paths, pixels, EXIF fields, or the selected pin's exact coordinates. Attributable areas may still be sensitive. Uncheck it to stop new requests and return to the offline grid; an already-sent request cannot be recalled.

- Native HTTPS submits only visible, settled-view XYZ tiles (zoom ≤19) to tile.openstreetmap.org with the app's own User-Agent. There is no offline download, background prefetch, arbitrary endpoint, cookie, or redirect.
- While tiles load or if the service is unavailable, the grid remains under the photos. The [OpenStreetMap attribution](https://www.openstreetmap.org/copyright) is visible while the layer is enabled. Use of the public tile service follows its [usage policy](https://operations.osmfoundation.org/policies/tiles/) and [privacy policy](https://osmfoundation.org/wiki/Privacy_Policy).
- The separate cache at %LOCALAPPDATA%\3wa_mypicsee\osm-tiles.sqlite3 stores previously viewed tile IDs/images and HTTP validators locally, bounded to about 96 MiB of tile data and 2,048 entries. It is not the GPS cache and can reveal viewed areas. To remove it, close PicSee and remove that cache file. Online mode honors expiry/revalidation; a fresh cache hit avoids a provider request. There is no offline mode using cached OSM tiles.
