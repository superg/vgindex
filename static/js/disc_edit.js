// Inline repeatable field (serial, edition, barcode)
function addInlineEntry(containerId, name) {
    var container = document.getElementById(containerId);
    var inputs = Array.prototype.filter.call(container.querySelectorAll('input'), function (input) {
        return input.name === name;
    });
    var last = inputs.length ? inputs[inputs.length - 1] : null;
    if (last && last.value.trim() === '') {
        last.focus();
        return;
    }
    var input = document.createElement('input');
    input.type = 'text';
    input.name = name;
    if (last) {
        input.style.width = last.style.width;
    }
    var annotation = container.querySelector('.review-field-annotation');
    if (annotation) {
        container.insertBefore(input, annotation);
    } else {
        container.appendChild(input);
    }
    if (name === 'edition') {
        attachEditionSelector(input);
    } else if (isIndependentlySizedInlineField(name)) {
        attachIndependentInlineResize(input);
    }
    input.focus();
}

function attachEditionSelector(input) {
    if (!input) return;
    input.removeAttribute('list');
    input.setAttribute('autocomplete', 'off');
    var group = ensureEditionSelectorGroup(input);
    if (input.dataset && input.dataset.editionSelector === 'true') return;
    if (input.dataset) input.dataset.editionSelector = 'true';

    var select = group.querySelector('select.edition-suggestion-select');
    if (!select || !select.classList || !select.classList.contains('edition-suggestion-select')) {
        select = document.createElement('select');
        select.className = 'edition-suggestion-select';
        select.setAttribute('aria-label', 'Choose edition');
        select.tabIndex = 0;
        group.appendChild(select);
    }

    populateEditionSelect(select);
    select.addEventListener('change', function () {
        var selectedEdition = select.value;
        if (!selectedEdition) return;
        input.value = selectedEdition;
        select.value = '';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        fitInlineInputSoon(input);
        input.focus();
    });
    attachIndependentInlineResize(input);
}

function ensureEditionSelectorGroup(input) {
    var parent = input.parentNode;
    if (parent && parent.classList && parent.classList.contains('edition-value-picker')) {
        return parent;
    }

    var group = document.createElement('span');
    group.className = 'edition-value-picker';
    parent.insertBefore(group, input);
    group.appendChild(input);
    return group;
}

function currentSystemEditionSuggestions() {
    var sysSel = document.getElementById('system-select');
    if (!sysSel || typeof EDITION_SUGGESTIONS === 'undefined') return [];
    return EDITION_SUGGESTIONS[sysSel.value] || [];
}

function populateEditionSelect(select) {
    if (!select) return;

    select.innerHTML = '';

    var blank = document.createElement('option');
    blank.value = '';
    blank.textContent = '';
    select.appendChild(blank);

    var suggestions = currentSystemEditionSuggestions();
    for (var i = 0; i < suggestions.length; i++) {
        var edition = suggestions[i];
        var option = document.createElement('option');
        option.value = edition;
        option.textContent = edition;
        select.appendChild(option);
    }

    select.value = '';
}

function initEditionSelectors() {
    document.querySelectorAll('input[name="edition"]').forEach(function (input) {
        attachEditionSelector(input);
    });
}

function refreshEditionSelectors() {
    document.querySelectorAll('.edition-suggestion-select').forEach(function (select) {
        populateEditionSelect(select);
    });
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
function isAddMode() {
    return typeof IS_ADD_MODE !== 'undefined' && !!IS_ADD_MODE;
}

function ringStatusClass(status) {
    if (status === 'added') return 'item-added';
    if (status === 'removed') return 'item-removed';
    if (status === 'changed') return 'item-changed';
    return '';
}

function ringInputClass(baseClass, status) {
    var extra = ringStatusClass(status);
    return extra ? (baseClass + ' ' + extra) : baseClass;
}

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

function getRingLayers() {
    return getMaxLayers() + 1;
}

function ringLayerLabel(layerIndex, layerCount) {
    return layerIndex === layerCount - 1 ? 'LS' : 'L' + layerIndex;
}

function ringLayerLabelHtml(layerIndex, layerCount) {
    var label = ringLayerLabel(layerIndex, layerCount);
    var tooltip = '';
    if (label === 'L0') {
        tooltip = 'L0 = Data side ring code.';
    } else if (label === 'L1') {
        tooltip = 'L1 = Data side ring code, second row (for 2+ layer discs).';
    } else if (label === 'L2') {
        tooltip = 'L2 = Data side ring code, third row (for 3+ layer discs only).';
    } else if (label === 'LS') {
        tooltip = 'LS = Label side ring codes.';
    }
    if (!tooltip) return '<strong>' + label + '</strong>';
    return '<strong class="ring-layer-label-tooltip" tabindex="0" title="' + esc(tooltip) + '" aria-label="' + esc(tooltip) + '">' + label + '</strong>';
}

function systemHasFlag(flag) {
    var sel = document.getElementById('system-select');
    if (!sel || typeof SYSTEMS_HAS_FLAGS === 'undefined') return false;
    var flags = SYSTEMS_HAS_FLAGS[sel.value];
    return flags ? !!flags[flag] : false;
}

function systemHasOffsetExtra() {
    return systemHasFlag('has_offset_extra');
}

function mediaIsCd() {
    var sel = document.getElementById('media-select');
    if (!sel || typeof MEDIA_IS_CD === 'undefined') return false;
    return !!MEDIA_IS_CD[sel.value];
}

function emptyLayer() {
    return { mastering_code: '', mastering_sid: '', toolstamps: '', mould_sids: '', additional_moulds: '' };
}

function emptyEntry(ml) {
    var layers = [];
    for (var i = 0; i < ml; i++) layers.push(emptyLayer());
    return { offset_value: '', offset_extra_value: '', sample_start: '', comment: '', layers: layers };
}

function ensureEmptyRingEntry() {
    var ml = getRingLayers();
    if (isAddMode()) {
        if (ringEntries.length === 0) {
            ringEntries.push(emptyEntry(ml));
        } else if (ringEntries.length > 1) {
            var firstNonEmpty = null;
            for (var i = 0; i < ringEntries.length; i++) {
                if (!isEntryEmpty(ringEntries[i])) {
                    firstNonEmpty = ringEntries[i];
                    break;
                }
            }
            ringEntries = [firstNonEmpty || ringEntries[0]];
        }
        return;
    }
    if (ringEntries.length === 0 || !isEntryEmpty(ringEntries[ringEntries.length - 1])) {
        ringEntries.push(emptyEntry(ml));
    }
}

function isEntryEmpty(entry) {
    if ((entry.offset_value || '').trim() !== '' || (entry.offset_extra_value || '').trim() !== '' || (entry.sample_start || '').trim() !== '' || (entry.comment || '').trim() !== '') return false;
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

    var ml = getRingLayers();

    // Pad all entries to current max layers
    for (var i = 0; i < ringEntries.length; i++) {
        if (!ringEntries[i].layers) ringEntries[i].layers = [];
        while (ringEntries[i].layers.length < ml) ringEntries[i].layers.push(emptyLayer());
    }

    var showOffset = mediaIsCd();
    var showSampleStart = showOffset && systemHasFlag('has_sample_start');
    var showOffsetExtra = showOffset && systemHasOffsetExtra();
    var showRemoveButton = !isAddMode();

    // Build header
    var hdr = '<tr><th>#</th>';
    if (ml > 1) hdr += '<th></th>';
    hdr += '<th>Mastering Code</th><th>Mastering SID</th><th>Toolstamps</th><th>Mould SIDs</th><th>Additional Moulds</th>';
    if (showOffset) hdr += '<th>Offset</th>';
    if (showOffsetExtra) hdr += '<th>Extra Offset</th>';
    if (showSampleStart) hdr += '<th>Sample Start</th>';
    hdr += '<th>Comment</th>';
    if (showRemoveButton) hdr += '<th></th>';
    hdr += '</tr>';
    thead.innerHTML = hdr;

    // Build rows
    tbody.innerHTML = '';
    for (var ei = 0; ei < ringEntries.length; ei++) {
        var entry = ringEntries[ei];
        var entryHighlight = (typeof RING_HIGHLIGHTS !== 'undefined' && RING_HIGHLIGHTS && RING_HIGHLIGHTS[ei]) ? RING_HIGHLIGHTS[ei] : null;
        var entryStatus = '';
        if (typeof entryHighlight === 'string') {
            entryStatus = entryHighlight;
            entryHighlight = null;
        } else if (entryHighlight && typeof entryHighlight === 'object' && typeof entryHighlight.entry === 'string') {
            entryStatus = entryHighlight.entry;
        }
        for (var li = 0; li < ml; li++) {
            var l = entry.layers[li] || emptyLayer();
            var tr = document.createElement('tr');
            tr.dataset.entry = ei;
            tr.dataset.layer = li;
            if (li === 0 && ei > 0) tr.className = 'ring-group-start';
            if (ei % 2 === 1) tr.classList.add('ring-entry-even');
            if (entryStatus) {
                var rowClass = ringStatusClass(entryStatus);
                if (rowClass) tr.classList.add(rowClass);
            }

            var layerHighlight = null;
            if (entryHighlight && Array.isArray(entryHighlight.layers) && entryHighlight.layers[li]) {
                layerHighlight = entryHighlight.layers[li];
            }

            var cells = '';
            if (li === 0) {
                cells += '<td class="entry-num"' + (ml > 1 ? ' rowspan="' + ml + '"' : '') + '>' + (ei + 1) + '</td>';
            }
            if (ml > 1) cells += '<td>' + ringLayerLabelHtml(li, ml) + '</td>';
            cells += '<td><input type="text" class="' + ringInputClass('ring-mc', layerHighlight && layerHighlight.mastering_code) + '" value="' + esc(l.mastering_code || '') + '"></td>';
            cells += '<td><input type="text" class="' + ringInputClass('ring-ms', layerHighlight && layerHighlight.mastering_sid) + '" value="' + esc(l.mastering_sid || '') + '"></td>';
            cells += '<td><input type="text" class="' + ringInputClass('ring-tools', layerHighlight && layerHighlight.toolstamps) + '" value="' + esc(l.toolstamps || '') + '"></td>';
            cells += '<td><input type="text" class="' + ringInputClass('ring-moulds', layerHighlight && layerHighlight.mould_sids) + '" value="' + esc(l.mould_sids || '') + '"></td>';
            cells += '<td><input type="text" class="' + ringInputClass('ring-addmoulds', layerHighlight && layerHighlight.additional_moulds) + '" value="' + esc(l.additional_moulds || '') + '"></td>';

            if (li === 0) {
                var rs = ml > 1 ? ' rowspan="' + ml + '"' : '';
                if (showOffset) cells += '<td' + rs + '><input type="text" class="' + ringInputClass('ring-offset', entryHighlight && entryHighlight.offset_value) + '" value="' + esc(entry.offset_value || '') + '"></td>';
                if (showOffsetExtra) cells += '<td' + rs + '><input type="text" class="' + ringInputClass('ring-offset-extra', entryHighlight && entryHighlight.offset_extra_value) + '" value="' + esc(entry.offset_extra_value || '') + '"></td>';
                if (showSampleStart) cells += '<td' + rs + '><input type="text" class="' + ringInputClass('ring-sample-start', entryHighlight && entryHighlight.sample_start) + '" value="' + esc(entry.sample_start || '') + '"></td>';
                cells += '<td' + rs + '><input type="text" class="' + ringInputClass('ring-comment', entryHighlight && entryHighlight.comment) + '" value="' + esc(entry.comment || '') + '"></td>';
                if (showRemoveButton) cells += '<td' + rs + '><button type="button" class="outline secondary remove-entry" onclick="removeRingEntry(' + ei + ')">&times;</button></td>';
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
    if (isAddMode()) {
        var firstInput = document.querySelector('#ring-tbody tr[data-entry="0"] input');
        if (firstInput) firstInput.focus();
        return;
    }
    saveRingFromDom();
    var last = ringEntries[ringEntries.length - 1];
    if (last && isEntryEmpty(last)) {
        var firstInput = document.querySelector('#ring-tbody tr[data-entry="' + (ringEntries.length - 1) + '"] input');
        if (firstInput) firstInput.focus();
        return;
    }
    ringEntries.push(emptyEntry(getRingLayers()));
    renderRingEntries();
    applyRingColumnWidths();
    var newFirst = document.querySelector('#ring-tbody tr[data-entry="' + (ringEntries.length - 1) + '"] input');
    if (newFirst) newFirst.focus();
}

function saveRingFromDom() {
    var tbody = document.getElementById('ring-tbody');
    if (!tbody) return;
    var ml = getRingLayers();
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
        var off = tr.querySelector('.ring-offset'); if (off) ringEntries[ei].offset_value = off.value;
        var offExtra = tr.querySelector('.ring-offset-extra'); if (offExtra) ringEntries[ei].offset_extra_value = offExtra.value;
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

function applySystemFieldVisibility() {
    var sysSel = document.getElementById('system-select');
    if (!sysSel || typeof SYSTEMS_HAS_FLAGS === 'undefined') return;
    var flags = SYSTEMS_HAS_FLAGS[sysSel.value];
    if (!flags) return;
    document.querySelectorAll('[data-field-flag]').forEach(function (el) {
        var flag = el.getAttribute('data-field-flag');
        var show = flags[flag] !== undefined ? !!flags[flag] : true;
        var wasHidden = el.style.display === 'none';
        el.style.display = show ? '' : 'none';
        el.querySelectorAll('input, select, textarea').forEach(function (ctrl) {
            ctrl.disabled = !show;
        });
        if (show && wasHidden) {
            el.querySelectorAll('textarea.auto-expand').forEach(function (ta) {
                autoExpand(ta);
            });
            el.querySelectorAll('.inline-field-values').forEach(function (c) {
                fitInlineGroup(c);
            });
        }
    });
}

function applyMediaFieldVisibility() {
    var mediaSel = document.getElementById('media-select');
    if (!mediaSel || typeof MEDIA_HAS_PIC === 'undefined' || typeof MEDIA_IS_CD === 'undefined') return;
    var code = mediaSel.value;
    var showPic = !!MEDIA_HAS_PIC[code];
    var showErrorCount = !!MEDIA_IS_CD[code];
    document.querySelectorAll('[data-media-flag="has_pic"]').forEach(function (el) {
        var wasHidden = el.style.display === 'none';
        el.style.display = showPic ? '' : 'none';
        el.querySelectorAll('input, select, textarea').forEach(function (ctrl) {
            ctrl.disabled = !showPic;
        });
        if (showPic && wasHidden) {
            el.querySelectorAll('textarea.auto-expand').forEach(function (ta) {
                autoExpand(ta);
            });
        }
    });
    var errorCountField = document.getElementById('error-count-field');
    if (!errorCountField) {
        var errorInput = document.querySelector('input[name="error_count"]');
        errorCountField = errorInput ? errorInput.closest('.inline-field') : null;
    }
    if (errorCountField) {
        var wasHidden = errorCountField.style.display === 'none';
        errorCountField.style.display = showErrorCount ? '' : 'none';
        errorCountField.querySelectorAll('input, select, textarea').forEach(function (ctrl) {
            ctrl.disabled = !showErrorCount;
        });
        if (showErrorCount && wasHidden) {
            errorCountField.querySelectorAll('.inline-field-values').forEach(function (c) {
                fitInlineGroup(c);
            });
        }
    }
}

function applyCueRules() {
    var mediaSel = document.getElementById('media-select');
    var cueField = document.getElementById('cue-field');
    if (!mediaSel || !cueField || typeof MEDIA_IS_CD === 'undefined') return;
    var isBin = !!MEDIA_IS_CD[mediaSel.value];
    var wasHidden = cueField.style.display === 'none';
    cueField.style.display = isBin ? '' : 'none';
    var textareas = cueField.querySelectorAll('textarea');
    textareas.forEach(function (ta) {
        ta.disabled = !isBin && !ta.readOnly;
        if (isBin && wasHidden) {
            autoExpand(ta);
        }
    });
}

function refreshMediaDependentUi() {
    applyMediaFieldVisibility();
    applyCueRules();
    renderRingEntries();
    fitRingColumns();
    renderLayerbreaks();
    fitAllInlineGroups();
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
var INDEPENDENT_INLINE_FIELDS = {
    'serial': true,
    'edition': true,
    'barcode': true
};

var DEFAULT_WIDTHS = {
    'serial': 12,
    'edition': 10,
    'barcode': 16,
    'version': 6,
    'error_count': 4,
    'exe_date': 10,
    'layerbreak': 8,
    'protection_key_disc_key': 32,
    'protection_key_disc_id': 32
};

function defaultWidthPx(name, font) {
    var chars = DEFAULT_WIDTHS[name];
    if (!chars) return MIN_INPUT_WIDTH;
    return measureText('M'.repeat(chars), font) + INPUT_PADDING;
}

function isIndependentlySizedInlineField(name) {
    return !!INDEPENDENT_INLINE_FIELDS[name];
}

function contentWidthPx(input) {
    var font = getInputFont(input);
    var w = measureText(input.value, font) + INPUT_PADDING;
    if (w > MIN_INPUT_WIDTH) return w;
    return defaultWidthPx(input.name, font);
}

function fitInlineInput(input) {
    if (!input) return;
    input.style.width = contentWidthPx(input) + 'px';
}

function fitInlineInputSoon(input) {
    fitInlineInput(input);
    if (window.requestAnimationFrame) {
        window.requestAnimationFrame(function () {
            fitInlineInput(input);
        });
    }
}

function attachIndependentInlineResize(input) {
    if (!input || !isIndependentlySizedInlineField(input.name)) return;
    if (input.dataset && input.dataset.inlineResize === 'true') return;
    if (input.dataset) input.dataset.inlineResize = 'true';
    input.addEventListener('input', function () {
        fitInlineInput(input);
    });
    input.addEventListener('change', function () {
        fitInlineInput(input);
    });
}

function initIndependentInlineResizing() {
    document.querySelectorAll('.inline-field-values input').forEach(function (input) {
        attachIndependentInlineResize(input);
    });
}

function fitIndependentInlineInputs(container) {
    container.querySelectorAll('input').forEach(function (input) {
        if (isIndependentlySizedInlineField(input.name)) fitInlineInput(input);
    });
}

// Size most inline groups to their widest value; some repeatable fields size independently.
function fitInlineGroup(container) {
    var inputs = container.querySelectorAll('input[type="text"], input[type="number"]');
    if (inputs.length === 0) return;
    if (isIndependentlySizedInlineField(inputs[0].name)) {
        fitIndependentInlineInputs(container);
        return;
    }
    var font = getInputFont(inputs[0]);
    var maxW = MIN_INPUT_WIDTH;
    inputs.forEach(function (inp) {
        var w = measureText(inp.value, font) + INPUT_PADDING;
        if (w > maxW) maxW = w;
    });
    if (maxW <= MIN_INPUT_WIDTH) {
        var name = inputs[0].name;
        maxW = defaultWidthPx(name, font);
    }
    inputs.forEach(function (inp) {
        inp.style.width = maxW + 'px';
    });
}

function fitInlineGroupForInput(input) {
    if (!input) return;
    if (isIndependentlySizedInlineField(input.name)) {
        fitInlineInput(input);
        return;
    }
    var container = input.closest ? input.closest('.inline-field-values') : null;
    if (container) fitInlineGroup(container);
}

function fitAllInlineGroups() {
    document.querySelectorAll('.inline-field-values').forEach(function (container) {
        fitInlineGroup(container);
    });
}

// Size ring code table columns by measuring max content per column class
var _ringColWidths = {};
var RING_COL_CLASSES = ['ring-mc', 'ring-ms', 'ring-tools', 'ring-moulds', 'ring-addmoulds', 'ring-offset', 'ring-offset-extra', 'ring-sample-start', 'ring-comment'];

var RING_DEFAULT_WIDTHS = {
    'ring-mc': 22,
    'ring-ms': 10,
    'ring-tools': 5,
    'ring-moulds': 10,
    'ring-addmoulds': 14,
    'ring-offset': 4,
    'ring-offset-extra': 4,
    'ring-sample-start': 6,
    'ring-comment': 8
};

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
        if (maxW <= MIN_INPUT_WIDTH && RING_DEFAULT_WIDTHS[cls]) {
            maxW = measureText('M'.repeat(RING_DEFAULT_WIDTHS[cls]), font) + INPUT_PADDING;
        }
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
    filterMediaTypes();
    applySystemFieldVisibility();
    refreshMediaDependentUi();
    initRingEditor();
    renderLayerbreaks();
    initAutoExpand();
    initEditionSelectors();
    initIndependentInlineResizing();
    fitAllInlineGroups();
    fitRingColumns();

    var sysSel = document.getElementById('system-select');
    if (sysSel) {
        sysSel.addEventListener('change', function () {
            filterMediaTypes();
            applySystemFieldVisibility();
            refreshEditionSelectors();
            refreshMediaDependentUi();
        });
    }
    var mediaSel = document.getElementById('media-select');
    if (mediaSel) {
        mediaSel.addEventListener('change', function () {
            refreshMediaDependentUi();
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
