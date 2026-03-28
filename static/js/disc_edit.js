// Inline repeatable field (serial, edition, barcode)
function addInlineEntry(containerId, name) {
    var container = document.getElementById(containerId);
    var last = container.querySelector('input:last-of-type');
    if (last && last.value.trim() === '') {
        last.focus();
        return;
    }
    var input = document.createElement('input');
    input.type = 'text';
    input.name = name;
    container.appendChild(input);
    input.focus();
}

// Vertical repeatable array field (layerbreaks)
function addArrayEntry(containerId, name) {
    var container = document.getElementById(containerId);
    var div = document.createElement('div');
    div.className = 'array-entry';
    var input = document.createElement('input');
    input.type = 'text';
    input.name = name;
    var btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'outline secondary remove-entry';
    btn.textContent = '\u00D7';
    btn.onclick = function () { div.remove(); };
    div.appendChild(input);
    div.appendChild(btn);
    container.appendChild(div);
    input.focus();
}

// Ring code editor — table-based, matching disc view layout
var ringEntries = [];

function initRingEditor() {
    ringEntries = (typeof RING_CODES !== 'undefined' && Array.isArray(RING_CODES)) ? RING_CODES : [];
    ensureEmptyRingEntry();
    renderRingEntries();
}

function getMaxLayers() {
    var sel = document.getElementById('media-select');
    if (sel && typeof MEDIA_LAYERS !== 'undefined') {
        var code = sel.value;
        if (MEDIA_LAYERS[code] !== undefined) return MEDIA_LAYERS[code];
    }
    return (typeof MAX_LAYERS !== 'undefined') ? MAX_LAYERS : 1;
}

function emptyLayer() {
    return { mastering_code: '', mastering_sid: '', toolstamps: '', mould_sids: '', additional_moulds: '' };
}

function emptyEntry(ml) {
    var layers = [];
    for (var i = 0; i < ml; i++) layers.push(emptyLayer());
    return { offset: '', sample_start: '', comment: '', layers: layers };
}

function ensureEmptyRingEntry() {
    var ml = getMaxLayers();
    if (ringEntries.length === 0 || !isEntryEmpty(ringEntries[ringEntries.length - 1])) {
        ringEntries.push(emptyEntry(ml));
    }
}

function isEntryEmpty(entry) {
    if ((entry.offset || '').trim() !== '' || (entry.sample_start || '').trim() !== '' || (entry.comment || '').trim() !== '') return false;
    if (!entry.layers) return true;
    for (var i = 0; i < entry.layers.length; i++) {
        var l = entry.layers[i];
        if ((l.mastering_code || '').trim() !== '' || (l.mastering_sid || '').trim() !== '' ||
            (l.toolstamps || '').trim() !== '' || (l.mould_sids || '').trim() !== '' ||
            (l.additional_moulds || '').trim() !== '') return false;
    }
    return true;
}

function renderRingEntries() {
    var thead = document.getElementById('ring-thead');
    var tbody = document.getElementById('ring-tbody');
    if (!thead || !tbody) return;

    var ml = getMaxLayers();

    // Pad all entries to current max layers
    for (var i = 0; i < ringEntries.length; i++) {
        if (!ringEntries[i].layers) ringEntries[i].layers = [];
        while (ringEntries[i].layers.length < ml) ringEntries[i].layers.push(emptyLayer());
    }

    var showSampleStart = (typeof HAS_SAMPLE_START !== 'undefined') && HAS_SAMPLE_START;

    // Build header
    var hdr = '<tr><th>#</th>';
    if (ml > 1) hdr += '<th></th>';
    hdr += '<th>Mastering Code</th><th>Mastering SID</th><th>Toolstamp(s)</th><th>Mould SID(s)</th><th>Additional Mould(s)</th>';
    hdr += '<th>Offset</th>';
    if (showSampleStart) hdr += '<th>Sample Start</th>';
    hdr += '<th>Comment</th><th></th></tr>';
    thead.innerHTML = hdr;

    // Build rows
    tbody.innerHTML = '';
    for (var ei = 0; ei < ringEntries.length; ei++) {
        var entry = ringEntries[ei];
        for (var li = 0; li < ml; li++) {
            var l = entry.layers[li] || emptyLayer();
            var tr = document.createElement('tr');
            tr.dataset.entry = ei;
            tr.dataset.layer = li;
            if (li === 0 && ei > 0) tr.className = 'ring-group-start';
            if (ei % 2 === 1) tr.classList.add('ring-entry-even');

            var cells = '';
            if (li === 0) {
                cells += '<td class="entry-num"' + (ml > 1 ? ' rowspan="' + ml + '"' : '') + '>' + (ei + 1) + '</td>';
            }
            if (ml > 1) cells += '<td><strong>L' + li + '</strong></td>';
            cells += '<td><input type="text" class="ring-mc" value="' + esc(l.mastering_code || '') + '"></td>';
            cells += '<td><input type="text" class="ring-ms" value="' + esc(l.mastering_sid || '') + '"></td>';
            cells += '<td><input type="text" class="ring-tools" value="' + esc(l.toolstamps || '') + '"></td>';
            cells += '<td><input type="text" class="ring-moulds" value="' + esc(l.mould_sids || '') + '"></td>';
            cells += '<td><input type="text" class="ring-addmoulds" value="' + esc(l.additional_moulds || '') + '"></td>';

            if (li === 0) {
                var rs = ml > 1 ? ' rowspan="' + ml + '"' : '';
                cells += '<td' + rs + '><input type="text" class="ring-offset" value="' + esc(entry.offset || '') + '"></td>';
                if (showSampleStart) cells += '<td' + rs + '><input type="text" class="ring-sample-start" value="' + esc(entry.sample_start || '') + '"></td>';
                cells += '<td' + rs + '><input type="text" class="ring-comment" value="' + esc(entry.comment || '') + '"></td>';
                cells += '<td' + rs + '><button type="button" class="outline secondary remove-entry" onclick="removeRingEntry(' + ei + ')">&times;</button></td>';
            }

            tr.innerHTML = cells;
            tbody.appendChild(tr);
        }
    }
}

function removeRingEntry(idx) {
    saveRingFromDom();
    ringEntries.splice(idx, 1);
    ensureEmptyRingEntry();
    renderRingEntries();
    applyRingColumnWidths();
}

function addRingEntry() {
    saveRingFromDom();
    var last = ringEntries[ringEntries.length - 1];
    if (last && isEntryEmpty(last)) {
        var firstInput = document.querySelector('#ring-tbody tr[data-entry="' + (ringEntries.length - 1) + '"] input');
        if (firstInput) firstInput.focus();
        return;
    }
    ringEntries.push(emptyEntry(getMaxLayers()));
    renderRingEntries();
    applyRingColumnWidths();
    var newFirst = document.querySelector('#ring-tbody tr[data-entry="' + (ringEntries.length - 1) + '"] input');
    if (newFirst) newFirst.focus();
}

function saveRingFromDom() {
    var tbody = document.getElementById('ring-tbody');
    if (!tbody) return;
    var ml = getMaxLayers();
    var rows = tbody.querySelectorAll('tr');
    rows.forEach(function (tr) {
        var ei = parseInt(tr.dataset.entry, 10);
        var li = parseInt(tr.dataset.layer, 10);
        if (isNaN(ei) || isNaN(li) || !ringEntries[ei]) return;
        var layer = ringEntries[ei].layers[li];
        if (!layer) return;
        var mc = tr.querySelector('.ring-mc'); if (mc) layer.mastering_code = mc.value;
        var ms = tr.querySelector('.ring-ms'); if (ms) layer.mastering_sid = ms.value;
        var tools = tr.querySelector('.ring-tools'); if (tools) layer.toolstamps = tools.value;
        var moulds = tr.querySelector('.ring-moulds'); if (moulds) layer.mould_sids = moulds.value;
        var addm = tr.querySelector('.ring-addmoulds'); if (addm) layer.additional_moulds = addm.value;
        var off = tr.querySelector('.ring-offset'); if (off) ringEntries[ei].offset = off.value;
        var ss = tr.querySelector('.ring-sample-start'); if (ss) ringEntries[ei].sample_start = ss.value;
        var cmt = tr.querySelector('.ring-comment'); if (cmt) ringEntries[ei].comment = cmt.value;
    });
}

function collectRingCodes() {
    saveRingFromDom();
    var result = [];
    for (var i = 0; i < ringEntries.length; i++) {
        if (!isEntryEmpty(ringEntries[i])) result.push(ringEntries[i]);
    }
    return result;
}

function val(parent, sel) {
    var el = parent.querySelector(sel);
    return el ? el.value : '';
}

function esc(s) {
    return String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// System <-> media type filtering
function filterMediaTypes() {
    var sysSel = document.getElementById('system-select');
    var mediaSel = document.getElementById('media-select');
    if (!sysSel || !mediaSel || typeof SYSTEMS_MEDIA === 'undefined') return;
    var allowed = SYSTEMS_MEDIA[sysSel.value];
    if (!allowed) return;
    var currentVal = mediaSel.value;
    var opts = mediaSel.options;
    for (var i = 0; i < opts.length; i++) {
        opts[i].hidden = allowed.indexOf(opts[i].value) === -1;
    }
    if (allowed.indexOf(currentVal) === -1 && allowed.length > 0) {
        mediaSel.value = allowed[0];
    }
}

// Auto-expand textareas to fit content
function autoExpand(textarea) {
    textarea.style.height = 'auto';
    textarea.style.height = textarea.scrollHeight + 'px';
}

function fitTextareaWidth(ta) {
    var lines = ta.value.split('\n');
    if (lines.length === 0 || (lines.length === 1 && lines[0] === '')) return;
    var font = getInputFont(ta);
    var maxW = 0;
    lines.forEach(function (line) {
        var w = measureText(line, font);
        if (w > maxW) maxW = w;
    });
    if (maxW > 0) {
        ta.style.width = (maxW + INPUT_PADDING) + 'px';
    }
}

function initAutoExpand() {
    document.querySelectorAll('textarea.fit-width').forEach(function (ta) {
        fitTextareaWidth(ta);
    });
    var areas = document.querySelectorAll('textarea.auto-expand');
    areas.forEach(function (ta) {
        autoExpand(ta);
        ta.addEventListener('input', function () { autoExpand(ta); });
    });
}

// Measure text width using a canvas context
var _measureCtx = null;
function measureText(text, font) {
    if (!_measureCtx) _measureCtx = document.createElement('canvas').getContext('2d');
    _measureCtx.font = font;
    return _measureCtx.measureText(text).width;
}

function getInputFont(input) {
    var cs = window.getComputedStyle(input);
    return cs.font || (cs.fontSize + ' ' + cs.fontFamily);
}

var MIN_INPUT_WIDTH = 40;
var INPUT_PADDING = 24;

// Size all inputs in an inline-field-values container to the max content width
function fitInlineGroup(container) {
    var inputs = container.querySelectorAll('input[type="text"], input[type="number"]');
    if (inputs.length === 0) return;
    var font = getInputFont(inputs[0]);
    var maxW = MIN_INPUT_WIDTH;
    inputs.forEach(function (inp) {
        var w = measureText(inp.value, font) + INPUT_PADDING;
        if (w > maxW) maxW = w;
    });
    inputs.forEach(function (inp) {
        inp.style.width = maxW + 'px';
    });
}

function fitAllInlineGroups() {
    document.querySelectorAll('.inline-field-values').forEach(function (container) {
        fitInlineGroup(container);
    });
}

// Size ring code table columns by measuring max content per column class
var _ringColWidths = {};
var RING_COL_CLASSES = ['ring-mc', 'ring-ms', 'ring-tools', 'ring-moulds', 'ring-addmoulds', 'ring-offset', 'ring-sample-start', 'ring-comment'];

function fitRingColumns() {
    var tbody = document.getElementById('ring-tbody');
    if (!tbody) return;
    _ringColWidths = {};
    RING_COL_CLASSES.forEach(function (cls) {
        var inputs = tbody.querySelectorAll('.' + cls);
        if (inputs.length === 0) return;
        var font = getInputFont(inputs[0]);
        var maxW = MIN_INPUT_WIDTH;
        inputs.forEach(function (inp) {
            var w = measureText(inp.value, font) + INPUT_PADDING;
            if (w > maxW) maxW = w;
        });
        _ringColWidths[cls] = maxW;
        inputs.forEach(function (inp) {
            inp.style.width = maxW + 'px';
        });
    });
}

function applyRingColumnWidths() {
    var tbody = document.getElementById('ring-tbody');
    if (!tbody) return;
    RING_COL_CLASSES.forEach(function (cls) {
        var w = _ringColWidths[cls];
        if (!w) return;
        tbody.querySelectorAll('.' + cls).forEach(function (inp) {
            inp.style.width = w + 'px';
        });
    });
}

// Layerbreaks — fixed inputs based on media type layers
function renderLayerbreaks() {
    var row = document.getElementById('layerbreak-row');
    var container = document.getElementById('layerbreak-list');
    if (!row || !container) return;
    var ml = getMaxLayers();
    var count = ml - 1;
    if (count <= 0) {
        row.style.display = 'none';
        return;
    }
    row.style.display = '';
    container.innerHTML = '';
    for (var i = 0; i < count; i++) {
        var input = document.createElement('input');
        input.type = 'text';
        input.name = 'layerbreak';
        input.value = (typeof LAYERBREAKS !== 'undefined' && LAYERBREAKS[i]) ? LAYERBREAKS[i] : '';
        container.appendChild(input);
    }
}

// Init
document.addEventListener('DOMContentLoaded', function () {
    initRingEditor();
    filterMediaTypes();
    renderLayerbreaks();
    initAutoExpand();
    fitAllInlineGroups();
    fitRingColumns();

    var sysSel = document.getElementById('system-select');
    if (sysSel) {
        sysSel.addEventListener('change', function () {
            filterMediaTypes();
            renderRingEntries();
            fitRingColumns();
        });
    }
    var mediaSel = document.getElementById('media-select');
    if (mediaSel) {
        mediaSel.addEventListener('change', function () {
            renderRingEntries();
            fitRingColumns();
            renderLayerbreaks();
        });
    }

    var form = document.getElementById('disc-edit-form');
    if (form) {
        form.addEventListener('submit', function () {
            var data = collectRingCodes();
            document.getElementById('ring-codes-json').value = JSON.stringify(data);
        });
    }
});
