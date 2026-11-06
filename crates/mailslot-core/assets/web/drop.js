/*
 * Mailslot drop page.
 *
 * (c) Copyright 2026 Liminal HQ, Scott Morris
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * Uploads use the resumable protocol: HEAD for the offset the server holds, then PUT of
 * the remaining bytes with Content-Range. The server's SHA-256 is authoritative; the page
 * pre-hashes small files only where WebCrypto exists (HTTPS or localhost) to skip files
 * the server already has.
 */
(function () {
	'use strict';

	var MAX_PARALLEL = 2;
	var MAX_AUTO_RETRIES = 3;
	var PREHASH_LIMIT = 64 * 1024 * 1024;
	var HAS_SUBTLE = !!(window.crypto && window.crypto.subtle);

	var $ = function (id) { return document.getElementById(id); };
	var zone = $('zone'), list = $('list'), sent = $('sent'), notice = $('notice');
	var maxBytes = 0;
	var items = [];
	var running = 0;
	var nextId = 1;

	function formatBytes(n) {
		if (n < 1024) { return n + ' B'; }
		var units = ['KB', 'MB', 'GB', 'TB'];
		var v = n / 1024, i = 0;
		while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
		return (v >= 100 ? v.toFixed(0) : v.toFixed(1)) + ' ' + units[i];
	}

	function formatTime(sec) {
		if (!isFinite(sec) || sec < 0) { return '…'; }
		sec = Math.round(sec);
		if (sec < 60) { return sec + ' s'; }
		var m = Math.floor(sec / 60), s = sec % 60;
		if (m < 60) { return m + ' min ' + s + ' s'; }
		return Math.floor(m / 60) + ' h ' + (m % 60) + ' min';
	}

	function showNotice(text, isError) {
		notice.textContent = text;
		notice.className = isError ? 'notice err' : 'notice';
		notice.hidden = !text;
	}

	function randomId() {
		var bytes = new Uint8Array(12);
		window.crypto.getRandomValues(bytes);
		return Array.prototype.map.call(bytes, function (b) { return ('0' + b.toString(16)).slice(-2); }).join('');
	}

	function hex(buffer) {
		return Array.prototype.map.call(new Uint8Array(buffer), function (b) { return ('0' + b.toString(16)).slice(-2); }).join('');
	}

	function request(method, url, headers, body, onUploadProgress, register) {
		return new Promise(function (resolve, reject) {
			var x = new XMLHttpRequest();
			x.open(method, url);
			Object.keys(headers || {}).forEach(function (k) { x.setRequestHeader(k, headers[k]); });
			if (onUploadProgress) { x.upload.onprogress = onUploadProgress; }
			x.onload = function () { resolve(x); };
			x.onerror = function () { reject(new Error('network')); };
			x.onabort = function () { reject(new Error('aborted')); };
			x.ontimeout = function () { reject(new Error('timeout')); };
			if (register) { register(x); }
			x.send(body || null);
		});
	}

	// ----- rows ---------------------------------------------------------------------

	function buildRow(item) {
		var li = document.createElement('li');
		li.className = 'item';
		var top = document.createElement('div');
		top.className = 'item-top';
		var name = document.createElement('span');
		name.className = 'name';
		name.textContent = item.label;
		var size = document.createElement('span');
		size.className = 'size';
		size.textContent = formatBytes(item.file.size);
		top.append(name, size);
		var bar = document.createElement('progress');
		bar.max = 1;
		bar.value = 0;
		bar.setAttribute('aria-label', 'Progress for ' + item.label);
		var meta = document.createElement('div');
		meta.className = 'meta';
		var status = document.createElement('span');
		status.className = 'status';
		status.setAttribute('role', 'status');
		var speed = document.createElement('span');
		var eta = document.createElement('span');
		meta.append(status, speed, eta);
		var hash = document.createElement('div');
		hash.className = 'hash';
		hash.hidden = true;
		var actions = document.createElement('div');
		actions.className = 'item-actions';
		li.append(top, bar, meta, hash, actions);
		item.ui = { li: li, bar: bar, status: status, speed: speed, eta: eta, hash: hash, actions: actions };
		return li;
	}

	function button(label, handler) {
		var b = document.createElement('button');
		b.type = 'button';
		b.className = 'small';
		b.textContent = label;
		b.addEventListener('click', handler);
		return b;
	}

	function render(item) {
		var u = item.ui;
		var frac = item.file.size ? Math.min(1, item.sent / item.file.size) : (item.state === 'done' ? 1 : 0);
		u.bar.value = item.state === 'done' || item.state === 'skipped' ? 1 : frac;
		u.li.className = 'item state-' + item.state;
		var text = {
			queued: 'Waiting',
			hashing: 'Checking file…',
			checking: 'Checking with the server…',
			uploading: Math.round(frac * 100) + '%',
			paused: 'Paused at ' + Math.round(frac * 100) + '%',
			done: 'Saved as ' + (item.savedAs || item.file.name),
			skipped: 'Already on the server, nothing to send',
			error: item.error || 'Failed'
		}[item.state];
		u.status.textContent = text;
		u.speed.textContent = item.state === 'uploading' && item.speed ? formatBytes(item.speed) + '/s' : '';
		u.eta.textContent = item.state === 'uploading' && item.speed ? formatTime((item.file.size - item.sent) / item.speed) + ' left' : '';
		if (item.sha256 && (item.state === 'done' || item.state === 'skipped')) {
			u.hash.hidden = false;
			u.hash.textContent = 'SHA-256 ' + item.sha256;
		}
		u.actions.textContent = '';
		if (item.state === 'uploading' || item.state === 'queued') {
			u.actions.append(button(item.state === 'queued' ? 'Cancel' : 'Pause', function () { pause(item); }));
		}
		if (item.state === 'paused') { u.actions.append(button('Resume', function () { resume(item); })); }
		if (item.state === 'error') { u.actions.append(button('Retry', function () { resume(item); })); }
		if ((item.state === 'done' || item.state === 'skipped') && item.sha256) {
			u.actions.append(button('Copy SHA-256', function () { copyText(item.sha256); }));
		}
	}

	function copyText(text) {
		if (navigator.clipboard && window.isSecureContext) {
			navigator.clipboard.writeText(text).then(function () { showNotice('SHA-256 copied.', false); }, function () { showNotice(text, false); });
		} else {
			// Plain HTTP has no clipboard API; show the value so it can be selected.
			showNotice(text, false);
		}
	}

	// ----- upload logic -------------------------------------------------------------

	function setState(item, state, error) {
		item.state = state;
		if (error !== undefined) { item.error = error; }
		render(item);
	}

	function pump() {
		while (running < MAX_PARALLEL) {
			var next = items.find(function (i) { return i.state === 'queued'; });
			if (!next) { return; }
			start(next);
		}
	}

	function start(item) {
		running++;
		item.attempts = 0;
		run(item).then(function () { running--; pump(); }, function () { running--; pump(); });
	}

	function finishError(item, text) {
		setState(item, 'error', text);
	}

	async function run(item) {
		try {
			item.cancelled = false;
			if (HAS_SUBTLE && item.expected === undefined && item.file.size <= PREHASH_LIMIT) {
				setState(item, 'hashing');
				item.expected = hex(await window.crypto.subtle.digest('SHA-256', await item.file.arrayBuffer()));
			}
			if (item.expected) {
				setState(item, 'checking');
				var have = await request('GET', 'have?sha256=' + item.expected);
				if (have.status === 200 && JSON.parse(have.responseText).have) {
					item.sha256 = item.expected;
					item.sent = item.file.size;
					setState(item, 'skipped');
					return;
				}
			}
			while (true) {
				if (item.cancelled) { return; }
				var offset = await queryOffset(item);
				if (offset > item.file.size) { offset = 0; }
				item.sent = offset;
				item.speed = 0;
				setState(item, 'uploading');
				var result = await send(item, offset);
				if (result === 'paused') { return; }
				if (result === 'done') { return; }
				if (result === 'retry') {
					item.attempts++;
					if (item.attempts > MAX_AUTO_RETRIES) { finishError(item, 'Connection problem. Check your network and try again.'); return; }
					item.ui.status.textContent = 'Reconnecting…';
					await new Promise(function (r) { setTimeout(r, Math.min(8000, 700 * Math.pow(2, item.attempts - 1))); });
					continue;
				}
				return;
			}
		} catch (e) {
			finishError(item, 'Could not read this file.');
		}
	}

	async function queryOffset(item) {
		try {
			var r = await request('HEAD', encodeURIComponent(item.file.name), { 'X-Mailslot-Upload-Id': item.uploadId });
			if (r.status === 200) {
				var v = parseInt(r.getResponseHeader('X-Mailslot-Offset') || '0', 10);
				return isNaN(v) ? 0 : v;
			}
		} catch (e) { /* the PUT will report the real problem */ }
		return 0;
	}

	function send(item, offset) {
		var size = item.file.size;
		var headers = { 'X-Mailslot-Upload-Id': item.uploadId, 'Accept': 'application/json' };
		if (size > 0) { headers['Content-Range'] = 'bytes ' + offset + '-' + (size - 1) + '/' + size; }
		if (item.expected) { headers['X-Mailslot-Expected-Sha256'] = item.expected; }
		var started = Date.now(), base = offset, lastTime = started, lastLoaded = 0;
		return request('PUT', encodeURIComponent(item.file.name), headers, item.file.slice(offset), function (e) {
			item.sent = base + e.loaded;
			var now = Date.now();
			if (now - lastTime >= 400) {
				var inst = (e.loaded - lastLoaded) / ((now - lastTime) / 1000);
				item.speed = item.speed ? item.speed * 0.6 + inst * 0.4 : inst;
				lastTime = now;
				lastLoaded = e.loaded;
			}
			render(item);
		}, function (x) { item.xhr = x; }).then(function (x) {
			item.xhr = null;
			if (x.status === 201) {
				var body = {};
				try { body = JSON.parse(x.responseText); } catch (e) { /* plain text fallback */ }
				item.savedAs = body.name || x.responseText;
				item.sha256 = body.sha256 || x.getResponseHeader('X-Mailslot-Sha256');
				item.sent = size;
				setState(item, 'done');
				return 'done';
			}
			if (x.status === 409) { return 'retry'; }
			if (x.status === 429 || x.status === 503 || x.status === 408 || x.status >= 500 && x.status !== 507) { return 'retry'; }
			var reasons = {
				404: 'This link is no longer valid.',
				410: 'This link has expired or reached its limit.',
				413: 'This file is larger than the limit' + (maxBytes ? ' of ' + formatBytes(maxBytes) : '') + '.',
				415: 'This type of file is not accepted.',
				422: 'The file changed in transit; try again.',
				507: 'The receiving computer is out of space or at its quota.'
			};
			finishError(item, reasons[x.status] || 'The server answered ' + x.status + '.');
			return 'error';
		}, function (e) {
			item.xhr = null;
			if (item.paused) { return 'paused'; }
			return 'retry';
		});
	}

	function pause(item) {
		if (item.state === 'queued') {
			item.cancelled = true;
			setState(item, 'paused');
			return;
		}
		item.paused = true;
		if (item.xhr) { item.xhr.abort(); }
		setState(item, 'paused');
	}

	function resume(item) {
		item.paused = false;
		item.error = '';
		setState(item, 'queued');
		pump();
	}

	// ----- adding files -------------------------------------------------------------

	function add(files) {
		var added = 0, rejected = [];
		files.forEach(function (entry) {
			var file = entry.file;
			if (maxBytes && file.size > maxBytes) {
				rejected.push(entry.label);
				return;
			}
			var item = { id: nextId++, file: file, label: entry.label, uploadId: randomId(), state: 'queued', sent: 0, speed: 0 };
			items.push(item);
			list.append(buildRow(item));
			render(item);
			added++;
		});
		sent.hidden = items.length === 0;
		if (rejected.length) {
			showNotice(rejected.length + ' file(s) skipped because they are larger than ' + formatBytes(maxBytes) + ': ' + rejected.slice(0, 3).join(', ') + (rejected.length > 3 ? '…' : ''), true);
		} else if (added) {
			showNotice('', false);
		}
		pump();
	}

	function fromFileList(fileList) {
		return Array.prototype.map.call(fileList, function (f) { return { file: f, label: f.webkitRelativePath || f.name }; });
	}

	function readEntries(reader) {
		return new Promise(function (resolve, reject) { reader.readEntries(resolve, reject); });
	}

	async function walk(entry, prefix, out) {
		if (entry.isFile) {
			await new Promise(function (resolve) {
				entry.file(function (f) { out.push({ file: f, label: prefix + f.name }); resolve(); }, resolve);
			});
		} else if (entry.isDirectory) {
			var reader = entry.createReader();
			while (true) {
				var batch = await readEntries(reader);
				if (!batch.length) { break; }
				for (var i = 0; i < batch.length; i++) { await walk(batch[i], prefix + entry.name + '/', out); }
			}
		}
	}

	async function fromDrop(dt) {
		var out = [];
		if (dt.items && dt.items.length && dt.items[0].webkitGetAsEntry) {
			var entries = [];
			for (var i = 0; i < dt.items.length; i++) {
				var e = dt.items[i].webkitGetAsEntry();
				if (e) { entries.push(e); }
			}
			for (var j = 0; j < entries.length; j++) { await walk(entries[j], '', out); }
			return out;
		}
		return fromFileList(dt.files);
	}

	// ----- wiring -------------------------------------------------------------------

	$('pick-files').addEventListener('click', function () { $('files').click(); });
	$('pick-folder').addEventListener('click', function () { $('folder').click(); });
	['files', 'folder'].forEach(function (id) {
		$(id).addEventListener('change', function (e) {
			add(fromFileList(e.target.files));
			e.target.value = '';
		});
	});
	zone.addEventListener('keydown', function (e) {
		if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); $('files').click(); }
	});
	var depth = 0;
	zone.addEventListener('dragenter', function (e) { e.preventDefault(); depth++; zone.classList.add('over'); });
	zone.addEventListener('dragover', function (e) { e.preventDefault(); });
	zone.addEventListener('dragleave', function () { depth = Math.max(0, depth - 1); if (!depth) { zone.classList.remove('over'); } });
	zone.addEventListener('drop', function (e) {
		e.preventDefault();
		depth = 0;
		zone.classList.remove('over');
		fromDrop(e.dataTransfer).then(add);
	});
	// A file dropped outside the zone should not navigate away from the page.
	['dragover', 'drop'].forEach(function (t) { window.addEventListener(t, function (e) { e.preventDefault(); }); });

	$('clear').addEventListener('click', function () {
		items = items.filter(function (i) {
			var finished = i.state === 'done' || i.state === 'skipped' || i.state === 'error';
			if (finished) { i.ui.li.remove(); }
			return !finished;
		});
		sent.hidden = items.length === 0;
	});

	window.addEventListener('beforeunload', function (e) {
		if (items.some(function (i) { return i.state === 'uploading'; })) { e.preventDefault(); e.returnValue = ''; }
	});

	$('hash-note').textContent = HAS_SUBTLE ? '' : 'This connection is not secure, so files are checked by the receiving computer instead of in your browser.';

	request('GET', 'health').then(function (x) {
		if (x.status !== 200) { return; }
		var h = JSON.parse(x.responseText);
		maxBytes = h.max_bytes || 0;
		var parts = [];
		if (maxBytes) { parts.push('Up to ' + formatBytes(maxBytes) + ' per file.'); }
		if (h.expires_at) { parts.push('Link expires ' + new Date(h.expires_at).toLocaleString() + '.'); }
		if (!h.accepting) { showNotice('This link is not accepting files right now.', true); }
		if (parts.length) { $('limits').textContent = parts.join(' '); }
	}, function () { /* the page still works if health is unreachable */ });
})();
