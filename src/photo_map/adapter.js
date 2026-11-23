(() => {
    "use strict";
    const send = (message) => window.ipc.postMessage(message);
    let map;
    let generation = 0;
    let records = [];
    let points, highlight, halo;

    let online = false;
    let epoch = 0;
    let activeEpoch = 0;
    let settleTimer = 0;
    let tileFrame = null;

    function pauseTiles() {
        clearTimeout(settleTimer);
        activeEpoch = 0;
        tileFrame = null;
        if (online) send("osm-pause");
    }

    function settleTiles() {
        if (!online) return;
        clearTimeout(settleTimer);
        epoch = (epoch + 1) >>> 0 || 1;
        const next = epoch;
        send("osm-view:" + next);
        // The native permission is sent before the subsequent frame asks for tiles.
        settleTimer = setTimeout(() => {
            if (!online || epoch !== next) return;
            activeEpoch = next;
            map.requestRender();
        }, 90);
    }

    function visibleTile(z, x, y) {
        const w = map.canvas.width, h = map.canvas.height;
        const { lon, lat, zoom, headingDeg, pitchDeg } = map.get3DCamera();
        const key = [w, h, lon, lat, zoom, headingDeg, pitchDeg, z].join("|");
        if (tileFrame?.key !== key) {
            const n = 2 ** z;
            const rad = lat * Math.PI / 180;
            tileFrame = { key, n, mvp: map._getCameraMvp(w, h),
                cx: (lon + 180) / 360 * n,
                cy: (1 - Math.asinh(Math.tan(rad)) / Math.PI) / 2 * n,
                size: map._getTileRenderState().size };
        }
        const { n, mvp, cx, cy, size } = tileFrame;
        const nearest = x + Math.round((cx - x - 0.5) / n) * n;
        // Shared wrapped texture can cover the left or the right copy of the world.
        for (const tileX of [nearest, nearest - n, nearest + n]) {
            const left = (tileX - cx) * size, top = (y - cy) * size;
            let polygon = [[left, top], [left + size, top],
                [left + size, top + size], [left, top + size]]
                .map(([px, pz]) => map._transformVec4(mvp, [px, 0, pz, 1]));
            for (let axis = 0; axis < 3 && polygon.length; axis++) {
                for (const sign of [1, -1]) {
                    const clipped = [];
                    for (let k = 0; k < polygon.length; k++) {
                        const a = polygon[k], b = polygon[(k + 1) % polygon.length];
                        const aa = a[3] + sign * a[axis], bb = b[3] + sign * b[axis];
                        if (aa >= 0) clipped.push(a);
                        if ((aa >= 0) !== (bb >= 0)) {
                            const t = aa / (aa - bb);
                            clipped.push(a.map((v, i) => v + t * (b[i] - v)));
                        }
                    }
                    polygon = clipped;
                }
            }
            if (polygon.length >= 3) return true;
        }
        return false;
    }

    const osm = {
        _id: "picsee-osm", _type: "dgSource", options: {},
        getZIndex: () => 1, getOpacity: () => 1, isVisible: () => true,
        getRetryCount: () => 2, getRetryDelay: () => 750,
        getTileUrl(x, y, z) {
            return online && activeEpoch > 0 && z <= 19 && visibleTile(z, x, y)
                ? "osm/" + activeEpoch + "/" + z + "/" + x + "/" + y + ".png"
                : null;
        },
    };

    function gridTile() {
        const grid = document.createElement("canvas");
        grid.width = grid.height = 256;
        const g = grid.getContext("2d");
        g.fillStyle = "#142831";
        g.fillRect(0, 0, 256, 256);
        for (const offset of [0, 64, 128, 192]) {
            g.fillStyle = offset === 0 ? "#365963" : "#233e48";
            g.fillRect(offset, 0, 2, 256);
            g.fillRect(0, offset, 256, 2);
        }
        return grid.toDataURL("image/png");
    }

    function fit() {
        if (records.length === 0) return;
        const reference = records[0].geometry.coordinates[0];
        let west = Infinity, east = -Infinity, south = Infinity, north = -Infinity;
        for (const { geometry: { coordinates: [lon, lat] } } of records) {
            const near = reference + (((lon - reference + 180) % 360 + 360) % 360 - 180);
            west = Math.min(west, near);
            east = Math.max(east, near);
            south = Math.min(south, lat);
            north = Math.max(north, lat);
        }
        if (east - west < 0.000001 && north - south < 0.000001) {
            map.zoomToXY([reference, north], 15, 0);
        } else {
            map.fitBounds([west, north], [east, south], { duration: 0, padding: [28, 28, 28, 28], maxZoom: 17 });
        }
    }

    function updateSelection(id, recenter) {
        const record = Number.isSafeInteger(id) ? records[id] : undefined;
        highlight.setData(record ? [record] : []);
        halo.setData(record ? [record] : []);
        if (record && recenter) map.setCenter(record.geometry.coordinates);
    }

    try {
        const tile = gridTile();
        window._dm4_maps = [{ _id: "picsee-grid", getTileUrl: () => tile }];
        map = new Easymap("map", { mapType: "picsee-grid", scaleLineVisible: false });
        if (!map.gl) throw new Error("WebGL is not available");
        map.disable3D(0);
        points = new dgWebGLPoint([], "EPSG:4326", { radius: 11, color: "#56E0BD", zIndex: 1 });
        halo = new dgWebGLPoint([], "EPSG:4326", { radius: 18, color: "#101C28", zIndex: 2 });
        highlight = new dgWebGLPoint([], "EPSG:4326", { radius: 12, color: "#FFCB72", zIndex: 3 });
        // Click tests must go to the underlying cluster, even when a member is selected.
        halo.hitTest = highlight.hitTest = () => null;
        points.setCluster({ enabled: true, distance: 32 });
        points.onclick = (_props, _type, _xy, _record, _event, members) => {
            const ids = [...new Set((members || []).map(member => member.properties?.id))]
                .filter(id => Number.isSafeInteger(id) && id >= 0 && id < records.length);
            if (ids.length === 1) send("select:" + generation + ":" + ids[0]);
            else if (ids.length > 1) send("cluster:" + generation + ":" + ids.join(","));
        };
        map.addItem([points, halo, highlight]);
        map.attachEvent("movestart", pauseTiles);
        map.attachEvent("moveend", settleTiles);
        window.picseeMapOsm = (enabled) => {
            if (enabled === online) return;
            if (enabled) {
                online = true;
                document.getElementById("legend").textContent = "© OpenStreetMap contributors";
                map.addItem(osm);
                settleTiles();
            } else {
                pauseTiles();
                online = false;
                map.removeItem(osm);
                document.getElementById("legend").textContent = "離線格線 · 無地理底圖";
            }
        };
        window.picseeMapSet = (nextGeneration, coordinates, id) => {
            generation = nextGeneration;
            records = coordinates.map(([lon, lat], i) => ({
                type: "Feature",
                geometry: { type: "Point", coordinates: [lon, Math.max(-85, Math.min(85, lat))] },
                properties: { id: i },
            }));
            points.setData(records);
            updateSelection(id, false);
            fit();
        };
        window.picseeMapSelect = (expectedGeneration, id) => {
            if (expectedGeneration === generation) updateSelection(id, true);
        };
        window.addEventListener("pagehide", () => map.destroy(), { once: true });
        const resize = new ResizeObserver(() => {
            map.resize();
            if (online) { pauseTiles(); settleTiles(); }
        });
        resize.observe(document.getElementById("map"));
        send("ready");
    } catch (_) {
        document.getElementById("legend").textContent = "無法啟用互動地圖";
        send("failed");
    }
})();
