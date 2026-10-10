"use strict";

// Liquid Glass refraction for what floats over the panel (the source menu),
// after https://kube.io/blog/liquid-glass-css-svg/: a displacement map
// computed from a glass surface, read by an SVG filter used as the
// element's `backdrop-filter`, so the panel under its rim bends like light
// through a lens. Only Chromium (WebView2 on Windows) runs SVG filters as a
// backdrop filter; elsewhere `supported` is false and the CSS keeps a plain
// blur. The window's own glass comes from the system (`glass.rs`).

(() => {
  const SVG = "http://www.w3.org/2000/svg";
  /** Refractive index of the glass; air is 1. */
  const N = 1.5;
  /** Samples along the bezel, the resolution of the 8-bit map. */
  const SAMPLES = 127;

  const supported = /Chrome\//.test(navigator.userAgent);

  /** Apple's profile: a squircle, smoother into the flat top than a circle. */
  const surface = (/** @type {number} */ x) => Math.pow(1 - Math.pow(1 - x, 4), 1 / 4);

  /**
   * How far a ray moves sideways crossing the bezel, from the edge (0) to
   * the flat top (1), in pixels: one refraction at the surface, rays
   * orthogonal to the panel, Snell's law with air at 1.
   * @param {number} bezel Bezel width, px.
   * @param {number} thickness Glass height at the flat top, px.
   */
  function profile(bezel, thickness) {
    const out = new Float32Array(SAMPLES + 1);
    const d = 0.001;
    for (let i = 0; i <= SAMPLES; i++) {
      const x = Math.min(1 - d, Math.max(d, i / SAMPLES));
      const slope = ((surface(x + d) - surface(x - d)) / (2 * d)) * (thickness / bezel);
      const incidence = Math.atan(slope);
      const refracted = Math.asin(Math.sin(incidence) / N);
      out[i] = surface(x) * thickness * Math.tan(incidence - refracted);
    }
    return out;
  }

  /**
   * Signed distance to a rounded rectangle centred at 0, negative inside.
   * @param {number} px @param {number} py @param {number} hw @param {number} hh @param {number} r
   */
  function sdf(px, py, hw, hh, r) {
    const qx = Math.abs(px) - (hw - r);
    const qy = Math.abs(py) - (hh - r);
    const ox = Math.max(qx, 0);
    const oy = Math.max(qy, 0);
    return Math.hypot(ox, oy) + Math.min(Math.max(qx, qy), 0) - r;
  }

  /**
   * The displacement map of a rounded rectangle, as a data URL, and the
   * scale that turns its channels back into pixels.
   * @param {number} w @param {number} h @param {number} radius
   * @param {{ bezel: number, thickness: number }} glass
   */
  function displacementMap(w, h, radius, glass) {
    const bezel = Math.min(glass.bezel, radius, w / 2, h / 2);
    const steps = profile(bezel, glass.thickness);
    const max = Math.max(...steps) || 1;
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = /** @type {CanvasRenderingContext2D} */ (canvas.getContext("2d"));
    const img = ctx.createImageData(w, h);
    const hw = w / 2;
    const hh = h / 2;
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        const px = x + 0.5 - hw;
        const py = y + 0.5 - hh;
        const inside = -sdf(px, py, hw, hh, radius);
        let dx = 0;
        let dy = 0;
        if (inside >= 0 && inside < bezel) {
          // The outward normal is the distance field's gradient; the lens
          // pulls the background from further in, against it.
          const gx = sdf(px + 0.5, py, hw, hh, radius) - sdf(px - 0.5, py, hw, hh, radius);
          const gy = sdf(px, py + 0.5, hw, hh, radius) - sdf(px, py - 0.5, hw, hh, radius);
          const len = Math.hypot(gx, gy) || 1;
          const m = steps[Math.round((inside / bezel) * SAMPLES)] / max;
          dx = (-gx / len) * m;
          dy = (-gy / len) * m;
        }
        const i = (y * w + x) * 4;
        img.data[i] = 128 + dx * 127;
        img.data[i + 1] = 128 + dy * 127;
        img.data[i + 2] = 128;
        img.data[i + 3] = 255;
      }
    }
    ctx.putImageData(img, 0, 0);
    // feDisplacementMap moves by scale × (channel − 0.5): a full channel is half the scale.
    return { url: canvas.toDataURL(), scale: 2 * max };
  }

  /** @type {Map<string, { url: string, scale: number }>} */
  const maps = new Map();

  /** @type {SVGSVGElement | null} */
  let defs = null;

  /**
   * Gives `el` a refracting backdrop for its current size and corner radius.
   * Call it again when the element changes size.
   * @param {HTMLElement} el
   * @param {{ bezel?: number, thickness?: number, blur?: number }} [options]
   */
  function apply(el, options = {}) {
    if (!supported) return;
    const w = Math.round(el.offsetWidth);
    const h = Math.round(el.offsetHeight);
    if (!w || !h) return;
    const radius = Math.min(parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0, w / 2, h / 2);
    const glass = { bezel: options.bezel ?? 14, thickness: options.thickness ?? 22 };
    const key = `${w}x${h}r${radius}b${glass.bezel}t${glass.thickness}`;
    let map = maps.get(key);
    if (!map) {
      map = displacementMap(w, h, radius, glass);
      maps.set(key, map);
    }
    const { url, scale } = map;
    const id = `refract-${el.id || "glass"}`;
    // Same map, same filter: nothing to rebuild.
    if (el.dataset.refraction === key && defs?.querySelector(`#${id}`)) {
      el.style.setProperty("backdrop-filter", `url(#${id})`);
      return;
    }
    el.dataset.refraction = key;

    if (!defs) {
      defs = document.createElementNS(SVG, "svg");
      defs.setAttribute("aria-hidden", "true");
      defs.setAttribute("width", "0");
      defs.setAttribute("height", "0");
      defs.style.position = "absolute";
      document.body.append(defs);
    }
    defs.querySelector(`#${id}`)?.remove();
    const filter = document.createElementNS(SVG, "filter");
    filter.id = id;
    filter.setAttribute("color-interpolation-filters", "sRGB");
    filter.setAttribute("filterUnits", "userSpaceOnUse");
    for (const [k, v] of [["x", 0], ["y", 0], ["width", w], ["height", h]]) filter.setAttribute(String(k), String(v));

    const blur = document.createElementNS(SVG, "feGaussianBlur");
    blur.setAttribute("in", "SourceGraphic");
    blur.setAttribute("stdDeviation", String(options.blur ?? 3));
    blur.setAttribute("result", "blurred");
    const image = document.createElementNS(SVG, "feImage");
    image.setAttribute("href", url);
    for (const [k, v] of [["x", 0], ["y", 0], ["width", w], ["height", h]]) image.setAttribute(String(k), String(v));
    image.setAttribute("preserveAspectRatio", "none");
    image.setAttribute("result", "map");
    const displace = document.createElementNS(SVG, "feDisplacementMap");
    displace.setAttribute("in", "blurred");
    displace.setAttribute("in2", "map");
    displace.setAttribute("scale", String(scale));
    displace.setAttribute("xChannelSelector", "R");
    displace.setAttribute("yChannelSelector", "G");
    displace.setAttribute("result", "bent");
    const saturate = document.createElementNS(SVG, "feColorMatrix");
    saturate.setAttribute("in", "bent");
    saturate.setAttribute("type", "saturate");
    saturate.setAttribute("values", "1.6");
    filter.append(blur, image, displace, saturate);
    defs.append(filter);
    el.style.setProperty("backdrop-filter", `url(#${id})`);
    el.classList.add("refracting");
  }

  window.Refraction = { supported, apply };
})();
