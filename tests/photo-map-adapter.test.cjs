const assert = require("node:assert/strict");
const {readFileSync} = require("node:fs");
const {runInNewContext} = require("node:vm");

function start() {
    const messages = [], layers = [], moves = [], handlers = {};
    const timers = new Map();
    const legend = { textContent: "" };
    let tick = 0;
    class Easymap {
        gl = {};
        disable3D() {}
        canvas = { width: 420, height: 230 };
        attachEvent(name, fn) { handlers[name] = fn; }
        requestRender() {}
        get3DCamera() { return {lon: 0, lat: 0, zoom: 3, headingDeg: 0, pitchDeg: 85}; }
        _getCameraMvp() { return []; }
        _getTileRenderState() { return {size: 3.5}; }
        _transformVec4(_m, p) { return p; }
        addItem(items) { layers.push(...(Array.isArray(items) ? items : [items])); }
        removeItem(layer) { layers.splice(layers.indexOf(layer), 1); }
        fitBounds(...args) { moves.push(["fit", ...args]); }
        zoomToXY(...args) { moves.push(["zoom", ...args]); }
        setCenter(...args) { moves.push(["center", ...args]); }
    }
    class dgWebGLPoint {
        setCluster(options) { this.cluster = options; }
        setData(records) { this.records = records; }
    }
    const ctx = {
        Easymap,
        dgWebGLPoint,
        ResizeObserver: class { observe() {} },
        setTimeout(fn) { timers.set(++tick, fn); return tick; },
        clearTimeout(id) { timers.delete(id); },
        document: {
            createElement: () => ({
                getContext: () => ({ fillRect() {} }),
                toDataURL: () => "data:image/png;base64,offline-test",
            }),
            getElementById: (id) => id === "legend" ? legend : ({}),
        },
        ipc: { postMessage: (msg) => messages.push(msg) },
        addEventListener() {},
    };
    ctx.window = ctx;
    runInNewContext(readFileSync(require.resolve("../src/photo_map/adapter.js"), "utf8"), ctx);
    function flush() { const pending = [...timers.values()]; timers.clear(); pending.forEach(fn => fn()); }
    return {ctx, messages, layers, moves, handlers, flush, legend};
}

const {ctx, messages, layers, moves} = start();
assert.deepEqual(messages, ["ready"]);
assert.match(ctx._dm4_maps[0].getTileUrl(), /^data:image\/png;/);
assert.equal(layers[0].cluster.enabled, true);
ctx.picseeMapSet(8, [[120, 24.001], [120.002, 24], [120.002, 24]], 0);
assert.equal(layers[0].records.length, 3);
assert.equal(layers[2].records[0].properties.id, 0);
assert.equal(moves.at(-1)[0], "fit");
layers[0].onclick(null, null, null, null, null, [layers[0].records[0]]);
layers[0].onclick(null, null, null, null, null, [layers[0].records[1], layers[0].records[2]]);
assert.deepEqual(messages, ["ready", "select:8:0", "cluster:8:1,2"]);
ctx.picseeMapSelect(7, 2);
assert.equal(layers[2].records[0].properties.id, 0, "ignore stale selection");
ctx.picseeMapSelect(8, 2);
assert.equal(layers[2].records[0].properties.id, 2);
assert.equal(moves.at(-1)[0], "center");
ctx.picseeMapSelect(8, null);
assert.equal(layers[2].records.length, 0, "clear the highlight when the selected photo has no GPS");
assert.equal(layers[0].records.length, 3, "keep surveyed locations");
ctx.picseeMapSet(9, [], null);
assert.equal(layers[0].records.length, 0);
assert.equal(layers[2].records.length, 0);
console.log("PASS: offline adapter, selection both directions, duplicate-position cluster and no-GPS marker clear");

const osmView = start();
assert.equal(osmView.layers.length, 3, "no raster tiles until explicit opt-in");
osmView.ctx.picseeMapOsm(true);
assert.equal(osmView.legend.textContent, "© OpenStreetMap contributors");
const osm = osmView.layers.at(-1);
assert.equal(osm.getTileUrl(4, 4, 3), null, "no outgoing tile before the settled viewport");
osmView.flush();
assert.equal(osmView.messages.at(-1), "osm-view:1");
assert.equal(osm.getTileUrl(4, 4, 3), "osm/1/3/4/4.png");
assert.equal(osm.getTileUrl(0, 0, 3), null, "outside actual viewing frustum");
assert.equal(osm.getTileUrl(4, 4, 20), null, "never request unsupported zoom");
osmView.handlers.movestart();
assert.equal(osmView.messages.at(-1), "osm-pause");
assert.equal(osm.getTileUrl(4, 4, 3), null, "no new tiles during motion");
osmView.handlers.moveend();
osmView.flush();
assert.equal(osm.getTileUrl(4, 4, 3), "osm/2/3/4/4.png");
osmView.ctx.picseeMapOsm(false);
assert.equal(osmView.legend.textContent, "離線格線 · 無地理底圖");
assert.equal(osmView.layers.length, 3);
assert.equal(osm.getTileUrl(4, 4, 3), null);
console.log("PASS: OSM is opt-in, only active settled viewport can produce fixed local routes");
const html = readFileSync(require.resolve("../src/photo_map/map.html"), "utf8");
assert.match(html, /lang="zh-TW"/);
assert.match(html, /離線格線 · 無地理底圖/);
assert.match(html, /connect-src 'none'/);
console.log("PASS: zh-TW map chrome retains attribution and local-only CSP");