// Measures text fit, collisions, loop seams and reduced motion in one diagram, in the browser.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

// scripts/diagram-fit.py inlines a diagram into a page with this script and reads the JSON it
// writes into <pre id="diagram-fit">. Conventions the diagrams follow:
//
// - The root <svg> carries data-loop="<seconds>", the length of one animation loop.
// - A shape that holds text has an id and data-box. A <text> inside it has data-for="<id>".
//   A <text> without data-for is a free label and only has to stay clear of boxes and lines.
// - Connectors carry data-line. No visible text may sit on a visible line.
// - Everything animated sits inside an element that `prefers-reduced-motion: reduce` hides.
//
// With ?shot=1&t=<s>&scheme=<light|dark>&reduced=<0|1> the page only freezes that frame, for
// screenshots.

(function () {
	"use strict";

	const MARGIN = 4; // Minimum gap between a label and the edge of its box, in px.
	const VISIBLE = 0.05; // Opacity below which an element counts as hidden.
	const STEP = 0.25; // Seconds between sampled animation frames.
	const svg = document.querySelector("body > svg");
	const out = document.getElementById("diagram-fit");
	const styles = Array.from(svg.querySelectorAll("style")).map((s) => [s, s.textContent]);
	const loop = parseFloat(svg.getAttribute("data-loop") || "0");
	const texts = Array.from(svg.querySelectorAll("text"));
	const boxes = Array.from(svg.querySelectorAll("[data-box]"));
	const lines = Array.from(svg.querySelectorAll("[data-line]"));
	const anims = Array.from(svg.querySelectorAll("animate, animateTransform, animateMotion, set"));

	function setMode(scheme, reduced) {
		for (const [el, text] of styles) {
			el.textContent = text
				.replace(/\(\s*prefers-color-scheme\s*:\s*dark\s*\)/g, scheme === "dark" ? "all" : "not all")
				.replace(/\(\s*prefers-color-scheme\s*:\s*light\s*\)/g, scheme === "light" ? "all" : "not all")
				.replace(/\(\s*prefers-reduced-motion\s*:\s*reduce\s*\)/g, reduced ? "all" : "not all")
				.replace(/\(\s*prefers-reduced-motion\s*:\s*no-preference\s*\)/g, reduced ? "not all" : "all");
		}
	}

	function setTime(t) {
		svg.pauseAnimations();
		svg.setCurrentTime(t);
		for (const a of document.getAnimations()) {
			a.pause();
			a.currentTime = t * 1000;
		}
	}

	function setFont(variant) {
		for (const t of texts) {
			t.style.fontFamily = "";
			if (variant === "native") {
				continue;
			}
			const mono = /mono/i.test(getComputedStyle(t).fontFamily);
			const families = {
				dejavu: mono ? '"DejaVu Sans Mono", monospace' : '"DejaVu Sans", sans-serif',
				liberation: mono ? '"Liberation Mono", monospace' : '"Liberation Sans", Arial, sans-serif',
			};
			t.style.fontFamily = families[variant];
		}
	}

	function opacity(el) {
		let o = 1;
		for (let e = el; e && e !== svg.parentNode; e = e.parentElement) {
			const cs = getComputedStyle(e);
			if (cs.display === "none") {
				return 0;
			}
			o *= parseFloat(cs.opacity);
		}
		return getComputedStyle(el).visibility === "visible" ? o : 0;
	}

	function rendered(el) {
		for (let e = el; e && e !== svg.parentNode; e = e.parentElement) {
			if (getComputedStyle(e).display === "none") {
				return false;
			}
		}
		return true;
	}

	function rootBox(el) {
		const b = el.getBBox();
		const m = el.getCTM();
		const xs = [];
		const ys = [];
		for (const [x, y] of [[b.x, b.y], [b.x + b.width, b.y], [b.x, b.y + b.height], [b.x + b.width, b.y + b.height]]) {
			xs.push(m.a * x + m.c * y + m.e);
			ys.push(m.b * x + m.d * y + m.f);
		}
		return { x0: Math.min(...xs), y0: Math.min(...ys), x1: Math.max(...xs), y1: Math.max(...ys) };
	}

	function overlap(a, b) {
		const w = Math.min(a.x1, b.x1) - Math.max(a.x0, b.x0);
		const h = Math.min(a.y1, b.y1) - Math.max(a.y0, b.y0);
		return w > 0 && h > 0 ? w * h : 0;
	}

	function contains(outer, inner) {
		return inner.x0 >= outer.x0 && inner.x1 <= outer.x1 && inner.y0 >= outer.y0 && inner.y1 <= outer.y1;
	}

	function label(el) {
		const s = (el.textContent || "").trim().replace(/\s+/g, " ");
		return el.tagName === "text" ? `"${s.slice(0, 40)}"` : `<${el.tagName}${el.id ? " #" + el.id : ""}>`;
	}

	// Contrast ------------------------------------------------------------------------------

	function parseColour(c) {
		const m = /rgba?\(([^)]+)\)/.exec(c || "");
		if (!m) {
			return null;
		}
		const p = m[1].split(/[ ,/]+/).filter(Boolean).map(parseFloat);
		return { r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1 };
	}

	function luminance(c) {
		const f = (v) => {
			v /= 255;
			return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
		};
		return 0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b);
	}

	function contrast(a, b) {
		const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
		return (x + 0.05) / (y + 0.05);
	}

	// The solid colours a shape is painted with: its fill, or every stop of its gradient.
	function paints(el) {
		const cs = getComputedStyle(el);
		if (parseFloat(cs.fillOpacity) < 1 || parseFloat(cs.opacity) < 1) {
			return null;
		}
		const ref = /url\("?#([^")]+)"?\)/.exec(cs.fill);
		if (ref) {
			const grad = svg.querySelector("#" + CSS.escape(ref[1]));
			if (!grad) {
				return null;
			}
			const stops = Array.from(grad.querySelectorAll("stop")).map((s) => {
				const scs = getComputedStyle(s);
				return parseFloat(scs.stopOpacity) < 1 ? null : parseColour(scs.stopColor);
			});
			return stops.length && stops.every(Boolean) ? stops : null;
		}
		const c = parseColour(cs.fill);
		return c && c.a === 1 ? [c] : null;
	}

	// One frame ------------------------------------------------------------------------------

	const failures = new Map();
	const stats = { frames: 0, measurements: 0, worstX: Infinity, worstY: Infinity, worstXAt: "", worstYAt: "", minContrast: Infinity, minContrastAt: "" };
	const background = svg.querySelector("[data-bg]");

	function fail(key, message, where) {
		if (!failures.has(key)) {
			failures.set(key, `${message} (${where})`);
		}
	}

	function frame(where) {
		stats.frames++;
		const vb = svg.viewBox.baseVal;
		const shownBoxes = boxes.filter((b) => opacity(b) > VISIBLE).map((b) => ({ el: b, r: rootBox(b) }));
		const shownTexts = [];
		for (const t of texts) {
			if (opacity(t) <= VISIBLE || !(t.textContent || "").trim()) {
				continue;
			}
			const r = rootBox(t);
			shownTexts.push({ el: t, r });
			stats.measurements++;
			if (r.x0 < vb.x + MARGIN || r.x1 > vb.x + vb.width - MARGIN || r.y0 < vb.y + MARGIN || r.y1 > vb.y + vb.height - MARGIN) {
				fail(`edge:${label(t)}`, `${label(t)} runs off the canvas`, where);
			}
			const forId = t.getAttribute("data-for");
			let own = null;
			if (forId) {
				const box = svg.getElementById(forId);
				if (!box || !box.hasAttribute("data-box")) {
					fail(`nobox:${label(t)}`, `${label(t)} names data-for="${forId}", which is not a data-box`, where);
					continue;
				}
				if (opacity(box) <= VISIBLE) {
					fail(`hidden:${label(t)}`, `${label(t)} is visible while its box #${forId} is not`, where);
					continue;
				}
				own = box;
				const b = rootBox(box);
				const gx = Math.min(r.x0 - b.x0, b.x1 - r.x1);
				const gy = Math.min(r.y0 - b.y0, b.y1 - r.y1);
				if (gx < stats.worstX) {
					stats.worstX = gx;
					stats.worstXAt = `${label(t)} in #${forId}, ${where}`;
				}
				if (gy < stats.worstY) {
					stats.worstY = gy;
					stats.worstYAt = `${label(t)} in #${forId}, ${where}`;
				}
				if (gx < MARGIN || gy < MARGIN) {
					fail(`fit:${label(t)}`, `${label(t)} leaves ${gx.toFixed(1)} px across and ${gy.toFixed(1)} px down to the edge of #${forId} (needs ${MARGIN})`, where);
				}
				const ink = parseColour(getComputedStyle(t).fill);
				const under = paints(box);
				if (ink && ink.a === 1 && under) {
					for (const c of under) {
						const k = contrast(ink, c);
						if (k < stats.minContrast) {
							stats.minContrast = k;
							stats.minContrastAt = `${label(t)} on #${forId}, ${where}`;
						}
						if (k < 4.5) {
							fail(`contrast:${label(t)}:${where.split(",")[1]}`, `${label(t)} has contrast ${k.toFixed(2)} on #${forId} (needs 4.5)`, where);
						}
					}
				}
			} else if (background) {
				const ink = parseColour(getComputedStyle(t).fill);
				const under = paints(background);
				if (ink && ink.a === 1 && under && opacity(t) > 0.99) {
					for (const c of under) {
						const k = contrast(ink, c);
						if (k < stats.minContrast) {
							stats.minContrast = k;
							stats.minContrastAt = `${label(t)} on the background, ${where}`;
						}
						if (k < 4.5) {
							fail(`contrast:${label(t)}:${where.split(",")[1]}`, `${label(t)} has contrast ${k.toFixed(2)} on the background (needs 4.5)`, where);
						}
					}
				}
			}
			for (const b of shownBoxes) {
				if (b.el === own) {
					continue;
				}
				if (overlap(r, b.r) > 0.5 && !contains(b.r, r)) {
					fail(`cross:${label(t)}:${label(b.el)}`, `${label(t)} crosses the edge of ${label(b.el)}`, where);
				}
			}
		}
		for (let i = 0; i < shownTexts.length; i++) {
			for (let j = i + 1; j < shownTexts.length; j++) {
				if (overlap(shownTexts[i].r, shownTexts[j].r) > 1) {
					fail(`pair:${label(shownTexts[i].el)}:${label(shownTexts[j].el)}`, `${label(shownTexts[i].el)} overlaps ${label(shownTexts[j].el)}`, where);
				}
			}
		}
		for (const line of lines) {
			// A <use> draws a path from <defs>; sample that path in the <use>'s coordinates.
			const ref = line.getAttribute("href") || line.getAttribute("xlink:href");
			const geom = line.tagName === "use" && ref ? svg.getElementById(ref.slice(1)) : line;
			if (opacity(line) <= VISIBLE || !geom || typeof geom.getTotalLength !== "function") {
				continue;
			}
			const m = line.getCTM();
			const len = geom.getTotalLength();
			for (let d = 0; d <= len; d += 2) {
				const p = geom.getPointAtLength(d);
				const x = m.a * p.x + m.c * p.y + m.e;
				const y = m.b * p.x + m.d * p.y + m.f;
				for (const s of shownTexts) {
					if (x > s.r.x0 - 1 && x < s.r.x1 + 1 && y > s.r.y0 - 1 && y < s.r.y1 + 1) {
						fail(`line:${label(line)}:${label(s.el)}`, `a line (${label(line)}) runs through ${label(s.el)}`, where);
					}
				}
			}
		}
	}

	// Whole-loop checks ---------------------------------------------------------------------

	function first(list) {
		return list.split(";")[0].trim();
	}

	function last(list) {
		const parts = list.split(";").map((s) => s.trim()).filter((s) => s !== "");
		return parts[parts.length - 1];
	}

	function target(a) {
		const href = a.getAttribute("href") || a.getAttribute("xlink:href");
		return href ? svg.getElementById(href.slice(1)) : a.parentElement;
	}

	function seconds(v) {
		const m = /^([\d.]+)(ms|s)?$/.exec((v || "").trim());
		if (!m) {
			return NaN;
		}
		return parseFloat(m[1]) / (m[2] === "ms" ? 1000 : 1);
	}

	function loopChecks() {
		if (!(loop > 0)) {
			fail("loop", "the root <svg> has no data-loop length", "static");
			return;
		}
		for (const a of anims) {
			const where = `${a.tagName} ${a.getAttribute("attributeName") || ""} on ${label(target(a))}`;
			const dur = seconds(a.getAttribute("dur"));
			if (a.getAttribute("repeatCount") !== "indefinite") {
				fail(`repeat:${where}`, `${where} does not repeat indefinitely`, "static");
			}
			const begin = a.getAttribute("begin");
			if (begin && seconds(begin) !== 0) {
				fail(`begin:${where}`, `${where} begins at ${begin}; every animation starts at 0 so loops stay in step`, "static");
			}
			if (!(dur > 0) || Math.abs(loop / dur - Math.round(loop / dur)) > 1e-6) {
				fail(`dur:${where}`, `${where} lasts ${a.getAttribute("dur")}, which does not divide the ${loop} s loop`, "static");
			}
			const kt = a.getAttribute("keyTimes");
			const vals = a.getAttribute("values");
			if (kt && vals && kt.split(";").length !== vals.split(";").filter((s) => s.trim() !== "").length) {
				fail(`kt:${where}`, `${where} has ${kt.split(";").length} keyTimes for ${vals.split(";").length} values`, "static");
			}
			let jumps = false;
			if (a.hasAttribute("data-cyclic") || a.getAttribute("attributeName") === "stroke-dashoffset") {
				jumps = false;
			} else if (a.tagName === "animateMotion") {
				const kp = a.getAttribute("keyPoints");
				jumps = !kp || first(kp) !== last(kp);
			} else if (vals) {
				jumps = first(vals) !== last(vals);
			} else if (a.hasAttribute("from") || a.hasAttribute("to")) {
				jumps = a.getAttribute("from") !== a.getAttribute("to");
			}
			if (jumps) {
				const el = target(a);
				for (const t of [0, loop - 0.01]) {
					setTime(t);
					if (opacity(el) > 0.02) {
						fail(`seam:${where}`, `${where} ends on a different value than it starts and is visible at the loop point (t=${t.toFixed(2)})`, "static");
					}
				}
			}
		}
	}

	function reducedChecks() {
		setMode("light", true);
		setTime(loop > 0 ? loop / 3 : 0);
		for (const a of anims) {
			const el = target(a);
			if (el && rendered(el)) {
				fail(`rm:${label(el)}`, `${label(el)} is still animated under prefers-reduced-motion`, "reduced");
			}
		}
		for (const a of document.getAnimations()) {
			if (a.effect && a.effect.target && rendered(a.effect.target)) {
				fail(`rmcss:${label(a.effect.target)}`, `${label(a.effect.target)} has a CSS animation under prefers-reduced-motion`, "reduced");
			}
		}
	}

	function fontReport() {
		const probe = document.createElementNS("http://www.w3.org/2000/svg", "text");
		probe.textContent = "Mailslot mmmmmmmmmm iiiii WWW 0123456789";
		probe.setAttribute("font-size", "40");
		svg.appendChild(probe);
		const width = (family) => {
			probe.style.fontFamily = family;
			return probe.getComputedTextLength();
		};
		const present = [];
		for (const f of ["DejaVu Sans", "DejaVu Sans Mono", "Liberation Sans", "Liberation Mono", "Noto Sans", "Cantarell", "Arial"]) {
			if (width(`"${f}", serif`) !== width("serif") || width(`"${f}", monospace`) !== width("monospace")) {
				present.push(f);
			}
		}
		probe.remove();
		return present;
	}

	function run() {
		const q = new URLSearchParams(location.search);
		if (q.get("shot") === "1") {
			setMode(q.get("scheme") || "light", q.get("reduced") === "1");
			setTime(parseFloat(q.get("t") || "0"));
			document.body.style.background = q.get("scheme") === "dark" ? "#0d1117" : "#ffffff";
			return;
		}
		const fonts = fontReport();
		setMode("light", false);
		loopChecks();
		reducedChecks();
		const times = [];
		for (let t = 0; t < Math.max(loop, STEP); t += STEP) {
			times.push(Math.round(t * 1000) / 1000);
		}
		const passes = [];
		for (const scheme of ["light", "dark"]) {
			for (const t of times) {
				passes.push({ scheme, reduced: false, t });
			}
			passes.push({ scheme, reduced: true, t: 0 });
		}
		for (const variant of ["native", "dejavu", "liberation"]) {
			setFont(variant);
			for (const p of passes) {
				setMode(p.scheme, p.reduced);
				setTime(p.t);
				frame(`${variant} font, ${p.scheme}${p.reduced ? " reduced motion" : ""}, t=${p.t.toFixed(2)}s`);
			}
		}
		setFont("native");
		out.textContent = JSON.stringify({
			loop,
			fonts,
			texts: texts.length,
			boxes: boxes.length,
			lines: lines.length,
			animations: anims.length,
			stats,
			failures: Array.from(failures.values()),
		});
	}

	document.fonts.ready.then(() => {
		try {
			run();
		} catch (e) {
			out.textContent = JSON.stringify({ error: String(e && e.stack ? e.stack : e) });
		}
	});
})();
