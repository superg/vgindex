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
    } else if (name === 'bca_mastering') {
        input.style.width = '18ch';
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
        fitInlineInput(input);
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
        select.setAttribute('aria-label', 'Choose edition suggestion');
        select.title = 'For large lists, type in the edition field to filter suggestions.';
        select.tabIndex = 0;
        group.appendChild(select);
    }

    populateEditionSelect(select);
    input.addEventListener('input', function () {
        populateEditionSelectForInput(select);
    });
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

var MAX_EDITION_SELECT_OPTIONS = 200;

function populateEditionSelect(select) {
    populateEditionSelectOptions(select, false);
}

function populateEditionSelectForInput(select) {
    populateEditionSelectOptions(select, true);
}

function populateEditionSelectOptions(select, filterByInput) {
    if (!select) return;

    var options = document.createDocumentFragment();
    var suggestions = currentSystemEditionSuggestions();
    var visibleSuggestions = suggestions;
    if (suggestions.length > MAX_EDITION_SELECT_OPTIONS) {
        var group = select.closest('.edition-value-picker');
        var input = group && group.querySelector('input[name="edition"]');
        var query = filterByInput && input ? input.value.trim().toLowerCase() : '';
        visibleSuggestions = [];
        for (var i = 0; i < suggestions.length; i++) {
            if (!query || suggestions[i].toLowerCase().indexOf(query) !== -1) {
                visibleSuggestions.push(suggestions[i]);
                if (visibleSuggestions.length === MAX_EDITION_SELECT_OPTIONS) break;
            }
        }
    }

    var blank = document.createElement('option');
    blank.value = '';
    blank.textContent = '';
    options.appendChild(blank);

    for (var j = 0; j < visibleSuggestions.length; j++) {
        var edition = visibleSuggestions[j];
        var option = document.createElement('option');
        option.value = edition;
        option.textContent = edition;
        options.appendChild(option);
    }

    select.replaceChildren(options);
    select.selectedIndex = 0;
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

function attachSubmitAsSelector(input) {
    if (!input) return;
    input.removeAttribute('list');
    input.setAttribute('autocomplete', 'off');
    var group = ensureSubmitAsSelectorGroup(input);
    if (input.dataset && input.dataset.submitAsSelector === 'true') return;
    if (input.dataset) input.dataset.submitAsSelector = 'true';

    var select = group.querySelector('select.submit-as-user-select');
    if (!select || !select.classList || !select.classList.contains('submit-as-user-select')) {
        select = document.createElement('select');
        select.className = 'submit-as-user-select';
        select.setAttribute('aria-label', 'Choose submitter');
        select.tabIndex = 0;
        group.appendChild(select);
    }

    populateSubmitAsSelect(select);
    select.addEventListener('change', function () {
        var selectedUser = select.value;
        if (!selectedUser) return;
        input.value = selectedUser;
        select.value = '';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        input.focus();
    });
}

function ensureSubmitAsSelectorGroup(input) {
    var parent = input.parentNode;
    if (parent && parent.classList && parent.classList.contains('submit-as-picker')) {
        return parent;
    }

    var group = document.createElement('span');
    group.className = 'submit-as-picker';
    parent.insertBefore(group, input);
    group.appendChild(input);
    return group;
}

function populateSubmitAsSelect(select) {
    if (!select) return;

    select.innerHTML = '';

    var blank = document.createElement('option');
    blank.value = '';
    blank.textContent = '';
    select.appendChild(blank);

    var usernames = (typeof SUBMIT_AS_USERNAMES !== 'undefined' && Array.isArray(SUBMIT_AS_USERNAMES)) ? SUBMIT_AS_USERNAMES : [];
    for (var i = 0; i < usernames.length; i++) {
        var username = usernames[i];
        var option = document.createElement('option');
        option.value = username;
        option.textContent = username;
        select.appendChild(option);
    }

    select.value = '';
}

function initSubmitAsSelector() {
    attachSubmitAsSelector(document.getElementById('submit-as-input'));
}

var FLAG_LIST_STORAGE_KEY = 'vgindex.addDisc.showAllFlags';

function flagListPreference() {
    try {
        return window.localStorage && window.localStorage.getItem(FLAG_LIST_STORAGE_KEY) === 'true';
    } catch (e) {
        return false;
    }
}

function saveFlagListPreference(showAll) {
    try {
        if (window.localStorage) {
            window.localStorage.setItem(FLAG_LIST_STORAGE_KEY, showAll ? 'true' : 'false');
        }
    } catch (e) {
        // Ignore storage failures; the current-page toggle still works.
    }
}

function setFlagListExpanded(showAll) {
    var form = document.getElementById('disc-edit-form');
    var toggle = document.getElementById('flag-list-toggle');
    if (!form || !toggle) return;

    form.classList.toggle('show-all-flags', showAll);
    toggle.setAttribute('aria-expanded', showAll ? 'true' : 'false');
    toggle.textContent = showAll
        ? (toggle.dataset.showCommonLabel || 'Show popular regions and languages')
        : (toggle.dataset.showAllLabel || 'Show all regions and languages');
}

function initFlagListToggle() {
    var toggle = document.getElementById('flag-list-toggle');
    if (!toggle) return;

    setFlagListExpanded(flagListPreference());
    toggle.addEventListener('click', function () {
        var form = document.getElementById('disc-edit-form');
        var showAll = !(form && form.classList.contains('show-all-flags'));
        setFlagListExpanded(showAll);
        saveFlagListPreference(showAll);
    });
}

function initLanguageVariantSelection() {
    var languageGroup = document.querySelector('.flag-select-languages');
    if (!languageGroup) return;

    languageGroup.addEventListener('change', function (event) {
        var input = event.target;
        if (!input.matches('input[type="checkbox"][name="languages"][data-base-language]')) return;
        refreshLanguageVariantLocks();
    });

    refreshLanguageVariantLocks();
}

function refreshLanguageVariantLocks() {
    var languageGroup = document.querySelector('.flag-select-languages');
    if (!languageGroup) return;

    var checkboxes = Array.from(
        languageGroup.querySelectorAll('input[type="checkbox"][name="languages"]')
    );
    var section = languageGroup.closest('[data-add-requires-system-media]');
    var sectionHidden = !!section && section.style.display === 'none';

    Array.from(new Set(checkboxes
        .map(function (input) { return input.dataset.baseLanguage; })
        .filter(Boolean)))
        .forEach(function (baseLanguage) {
            var baseInput = checkboxes.find(function (input) {
                return !input.dataset.baseLanguage && input.value === baseLanguage;
            });
            if (!baseInput) return;

            var hasSelectedVariant = checkboxes.some(function (input) {
                return input.dataset.baseLanguage === baseLanguage && input.checked;
            });
            var baseSubmitInput = Array.from(
                languageGroup.querySelectorAll('[data-language-base-submit]')
            ).find(function (input) {
                return input.dataset.languageBaseSubmit === baseLanguage;
            });

            if (hasSelectedVariant) {
                baseInput.checked = true;
                baseInput.disabled = true;
                if (!baseSubmitInput) {
                    baseSubmitInput = document.createElement('input');
                    baseSubmitInput.type = 'hidden';
                    baseSubmitInput.name = 'languages';
                    baseSubmitInput.value = baseLanguage;
                    baseSubmitInput.dataset.languageBaseSubmit = baseLanguage;
                    languageGroup.appendChild(baseSubmitInput);
                }
                baseSubmitInput.disabled = sectionHidden;
            } else {
                baseInput.disabled = sectionHidden;
                if (baseSubmitInput) baseSubmitInput.remove();
            }
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
var ringEntryHighlights = [];
var ringCueVariantOpen = [];
function isAddMode() {
    return typeof IS_ADD_MODE !== 'undefined' && !!IS_ADD_MODE;
}

function canAddRingcodeEntries() {
    return typeof CAN_ADD_RINGCODE_ENTRIES !== 'undefined' && !!CAN_ADD_RINGCODE_ENTRIES;
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
    var initialHighlights = (typeof RING_HIGHLIGHTS !== 'undefined' && Array.isArray(RING_HIGHLIGHTS)) ? RING_HIGHLIGHTS : [];
    ringCueVariantOpen = ringEntries.map(function (entry) {
        return !!entry && typeof entry.cue_variant === 'string' && entry.cue_variant.trim() !== '';
    });
    ringEntryHighlights = ringEntries.map(function (_, idx) {
        return initialHighlights[idx] || null;
    });
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
    return '<strong class="ring-layer-label-tooltip" tabindex="0" data-ring-tooltip="' + esc(tooltip) + '" aria-label="' + esc(tooltip) + '">' + label + '</strong>';
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

function sbiAppliesToCurrentSelection() {
    return systemHasFlag('has_sbi') && mediaIsCd();
}

function emptyLayer() {
    return { mastering_code: '', mastering_sid: '', toolstamps: '', mould_sids: '', additional_moulds: '' };
}

function emptyEntry(ml) {
    var layers = [];
    for (var i = 0; i < ml; i++) layers.push(emptyLayer());
    return { offset_value: '', offset_extra_value: '', sample_start: '', comment: '', cue_variant: '', layers: layers };
}

function pushRingEntry(entry, highlight) {
    ringEntries.push(entry);
    ringEntryHighlights.push(highlight || null);
    ringCueVariantOpen.push(false);
}

function removeRingEntryState(idx) {
    ringEntries.splice(idx, 1);
    ringEntryHighlights.splice(idx, 1);
    ringCueVariantOpen.splice(idx, 1);
}

function ensureEmptyRingEntry() {
    var ml = getRingLayers();
    if (isAddMode()) {
        if (ringEntries.length === 0) {
            pushRingEntry(emptyEntry(ml), null);
        } else if (ringEntries.length > 1) {
            var keepIndex = 0;
            for (var i = 0; i < ringEntries.length; i++) {
                if (!isEntryEmpty(ringEntries[i])) {
                    keepIndex = i;
                    break;
                }
            }
            ringEntries = [ringEntries[keepIndex]];
            ringEntryHighlights = [ringEntryHighlights[keepIndex] || null];
            ringCueVariantOpen = [ringCueVariantOpen[keepIndex] || false];
        }
        return;
    }
    if (!canAddRingcodeEntries()) return;
    if (ringEntries.length === 0 || !isEntryEmpty(ringEntries[ringEntries.length - 1])) {
        pushRingEntry(emptyEntry(ml), null);
    }
}

function isEntryEmpty(entry) {
    if ((entry.offset_value || '').trim() !== '' || (entry.offset_extra_value || '').trim() !== '' || (entry.sample_start || '').trim() !== '' || (entry.comment || '').trim() !== '') return false;
    if (mediaIsCd() && (entry.cue_variant || '').trim() !== '') return false;
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
    var showCueVariant = showOffset && canAddRingcodeEntries() && !isAddMode();

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
    var columnCount = 1 + (ml > 1 ? 1 : 0) + 5 + (showOffset ? 1 : 0) +
        (showOffsetExtra ? 1 : 0) + (showSampleStart ? 1 : 0) + 1 +
        (showRemoveButton || showCueVariant ? 1 : 0);

    // Build rows
    tbody.innerHTML = '';
    for (var ei = 0; ei < ringEntries.length; ei++) {
        var entry = ringEntries[ei];
        var entryHighlight = ringEntryHighlights[ei] || null;
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
            var layerSeedPrefix = li === ml - 1 ? 'label_side_' : 'layer_' + li + '_';
            cells += '<td><input type="text" data-json-seed-key="' + layerSeedPrefix + 'mastering_code" class="' + ringInputClass('ring-mc', layerHighlight && layerHighlight.mastering_code) + '" value="' + esc(l.mastering_code || '') + '"></td>';
            cells += '<td><input type="text" data-json-seed-key="' + layerSeedPrefix + 'mastering_sid" class="' + ringInputClass('ring-ms', layerHighlight && layerHighlight.mastering_sid) + '" value="' + esc(l.mastering_sid || '') + '"></td>';
            cells += '<td><input type="text" data-json-seed-key="' + layerSeedPrefix + 'toolstamps" class="' + ringInputClass('ring-tools', layerHighlight && layerHighlight.toolstamps) + '" value="' + esc(l.toolstamps || '') + '"></td>';
            var mouldSidValue = l.mould_sids || '';
            var isMouldLayer = li === 0 || li === ml - 1;
            var mouldSeedPrefix = li === ml - 1 ? 'label_side_' : (li === 0 ? 'layer_0_' : '');
            var mouldSeedAttr = mouldSeedPrefix
                ? ' data-json-seed-key="' + mouldSeedPrefix + 'mould_sids"'
                : '';
            if (isMouldLayer || mouldSidValue.trim() !== '') {
                cells += '<td><input type="text"' + mouldSeedAttr + ' class="' + ringInputClass('ring-moulds', layerHighlight && layerHighlight.mould_sids) + '" value="' + esc(mouldSidValue || '') + '"></td>';
            } else {
                cells += '<td></td>';
            }
            var additionalMouldsValue = l.additional_moulds || '';
            var additionalMouldsSeedAttr = mouldSeedPrefix
                ? ' data-json-seed-key="' + mouldSeedPrefix + 'additional_moulds"'
                : '';
            if (isMouldLayer || additionalMouldsValue.trim() !== '') {
                cells += '<td><input type="text"' + additionalMouldsSeedAttr + ' class="' + ringInputClass('ring-addmoulds', layerHighlight && layerHighlight.additional_moulds) + '" value="' + esc(additionalMouldsValue || '') + '"></td>';
            } else {
                cells += '<td></td>';
            }

            if (li === 0) {
                var rs = ml > 1 ? ' rowspan="' + ml + '"' : '';
                if (showOffset) cells += '<td' + rs + '><input type="text" class="' + ringInputClass('ring-offset', entryHighlight && entryHighlight.offset_value) + '" value="' + esc(entry.offset_value || '') + '"></td>';
                if (showOffsetExtra) cells += '<td' + rs + '><input type="text" class="' + ringInputClass('ring-offset-extra', entryHighlight && entryHighlight.offset_extra_value) + '" value="' + esc(entry.offset_extra_value || '') + '"></td>';
                if (showSampleStart) cells += '<td' + rs + '><input type="text" class="' + ringInputClass('ring-sample-start', entryHighlight && entryHighlight.sample_start) + '" value="' + esc(entry.sample_start || '') + '"></td>';
                cells += '<td' + rs + '><input type="text" data-json-seed-key="ring_comment" class="' + ringInputClass('ring-comment', entryHighlight && entryHighlight.comment) + '" value="' + esc(entry.comment || '') + '"></td>';
                if (showRemoveButton || showCueVariant) {
                    cells += '<td' + rs + '>';
                    if (showRemoveButton) cells += '<button type="button" class="outline secondary remove-entry" onclick="removeRingEntry(' + ei + ')">&times;</button>';
                    if (showCueVariant) {
                        cells += '<button type="button" class="outline secondary ring-cue-variant-toggle" aria-expanded="' +
                            (!!ringCueVariantOpen[ei]) + '" onclick="toggleRingCueVariant(' + ei + ')">CUE Variant</button>';
                    }
                    cells += '</td>';
                }
            }

            tr.innerHTML = cells;
            tbody.appendChild(tr);
        }
        if (showCueVariant && ringCueVariantOpen[ei]) {
            var cueRow = document.createElement('tr');
            cueRow.className = 'ring-cue-variant-row';
            cueRow.dataset.entry = ei;
            cueRow.innerHTML = '<td colspan="' + columnCount + '"><label>CUE Variant' +
                '<textarea id="ring-cue-variant-' + ei + '" class="ring-cue-variant hex-scrollable" rows="5" spellcheck="false">' +
                esc(entry.cue_variant || '') + '</textarea></label></td>';
            tbody.appendChild(cueRow);
        }
    }
}

function toggleRingCueVariant(idx) {
    saveRingFromDom();
    ringCueVariantOpen[idx] = !ringCueVariantOpen[idx];
    renderRingEntries();
    applyRingColumnWidths();
    if (ringCueVariantOpen[idx]) {
        var textarea = document.getElementById('ring-cue-variant-' + idx);
        if (textarea) textarea.focus();
    }
}

function removeRingEntry(idx) {
    saveRingFromDom();
    removeRingEntryState(idx);
    ensureEmptyRingEntry();
    renderRingEntries();
    applyRingColumnWidths();
}

function addRingEntry() {
    if (!canAddRingcodeEntries()) return;
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
    pushRingEntry(emptyEntry(getRingLayers()), null);
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
        var cueVariant = tr.querySelector('.ring-cue-variant');
        if (cueVariant && !isNaN(ei) && ringEntries[ei]) {
            ringEntries[ei].cue_variant = cueVariant.value;
            return;
        }
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

function normalizeRingcodeWhitespace(value) {
    if (value === null || value === undefined) return '';
    return String(value).trim().replace(/[^\S\r\n]{2,}/g, '\t');
}

function normalizeRingLayerWhitespace(layer) {
    var out = {};
    for (var key in layer) {
        if (Object.prototype.hasOwnProperty.call(layer, key)) out[key] = layer[key];
    }
    ['mastering_code', 'mastering_sid', 'toolstamps', 'mould_sids', 'additional_moulds'].forEach(function (field) {
        out[field] = normalizeRingcodeWhitespace(out[field]);
    });
    return out;
}

function normalizeRingIntegerString(value) {
    if (value === null || value === undefined) return '';
    var text = String(value).trim();
    if (text === '') return '';
    if (!/^[+-]?\d+$/.test(text)) return String(value);

    var number = Number(text);
    if (!Number.isInteger(number) || number < -2147483648 || number > 2147483647) {
        return String(value);
    }
    return String(number);
}

function normalizeRingEntryForSubmission(entry) {
    var out = {};
    for (var key in entry) {
        if (Object.prototype.hasOwnProperty.call(entry, key) && key !== 'layers') out[key] = entry[key];
    }
    ['offset_value', 'offset_extra_value', 'sample_start'].forEach(function (field) {
        out[field] = normalizeRingIntegerString(out[field]);
    });
    out.cue_variant = mediaIsCd() ? String(out.cue_variant || '') : '';
    out.layers = [];
    var layers = Array.isArray(entry.layers) ? entry.layers : [];
    for (var i = 0; i < layers.length; i++) {
        out.layers.push(normalizeRingLayerWhitespace(layers[i] || {}));
    }
    return out;
}

function collectRingCodes() {
    saveRingFromDom();
    var result = [];
    for (var i = 0; i < ringEntries.length; i++) {
        if (!isEntryEmpty(ringEntries[i])) result.push(normalizeRingEntryForSubmission(ringEntries[i]));
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

function setFormFieldVisible(el, show) {
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
}

function addNeedsSystemMediaSelection() {
    if (!isAddMode()) return false;
    var sysSel = document.getElementById('system-select');
    var mediaSel = document.getElementById('media-select');
    return !sysSel || !mediaSel || !(sysSel.value || '').trim() || !(mediaSel.value || '').trim();
}

function releaseAddMinimalMode() {
    if (!isAddMode()) return;
    var form = document.getElementById('disc-edit-form');
    if (form) form.classList.remove('disc-add-minimal');
    document.querySelectorAll('[data-add-requires-system-media]').forEach(function (el) {
        setFormFieldVisible(el, true);
    });
}

function enforceAddMinimalMode() {
    if (!isAddMode()) return;
    var minimal = addNeedsSystemMediaSelection();
    var form = document.getElementById('disc-edit-form');
    if (form) form.classList.toggle('disc-add-minimal', minimal);
    if (!minimal) return;

    document.querySelectorAll('[data-add-requires-system-media]').forEach(function (el) {
        setFormFieldVisible(el, false);
    });
    document.querySelectorAll('[data-field-flag="has_universal_hash"]').forEach(function (el) {
        setFormFieldVisible(el, true);
    });
}

function applySystemFieldVisibility() {
    var sysSel = document.getElementById('system-select');
    if (!sysSel || typeof SYSTEMS_HAS_FLAGS === 'undefined') return;
    var flags = SYSTEMS_HAS_FLAGS[sysSel.value];
    if (!flags) return;
    document.querySelectorAll('[data-field-flag]').forEach(function (el) {
        var flag = el.getAttribute('data-field-flag');
        var show = flags[flag] !== undefined ? !!flags[flag] : true;
        if (flag === 'has_sbi') {
            show = show && mediaIsCd();
        }
        setFormFieldVisible(el, show);
    });
}

function applyMediaFieldVisibility() {
    var mediaSel = document.getElementById('media-select');
    if (!mediaSel || typeof MEDIA_HAS_PIC === 'undefined' || typeof MEDIA_IS_CD === 'undefined') return;
    var code = mediaSel.value;
    var extrasField = document.getElementById('extras-field');
    if (extrasField && typeof MEDIA_HAS_SS !== 'undefined') {
        var showExtras = !!MEDIA_HAS_SS[code] ||
            (systemHasFlag('has_extras') && !!MEDIA_IS_CD[code]);
        var wasHidden = extrasField.style.display === 'none';
        extrasField.style.display = showExtras ? '' : 'none';
        extrasField.querySelectorAll('input, select, textarea').forEach(function (ctrl) {
            ctrl.disabled = !showExtras && !ctrl.readOnly;
        });
        if (showExtras && wasHidden) {
            extrasField.querySelectorAll('textarea.auto-expand').forEach(function (ta) {
                autoExpand(ta);
            });
        }
    }
    var showPic = !!MEDIA_HAS_PIC[code];
    var showDmi = !!MEDIA_HAS_DMI[code] && systemHasFlag('has_dmi');
    var showErrorCount = !!MEDIA_IS_CD[code] && systemHasFlag('has_error_count');
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
    document.querySelectorAll('[data-media-flag="has_dmi"]').forEach(function (el) {
        setFormFieldVisible(el, showDmi);
    });
    document.querySelectorAll('[data-media-codes]').forEach(function (el) {
        var allowed = (el.getAttribute('data-media-codes') || '').split(/\s+/);
        setFormFieldVisible(el, allowed.indexOf(code) !== -1);
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
    document.querySelectorAll('[data-field-flag="has_sbi"]').forEach(function (el) {
        setFormFieldVisible(el, sbiAppliesToCurrentSelection());
    });
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
    saveRingFromDom();
    renderRingEntries();
    fitRingColumns();
    renderLayerbreaks();
    fitAllInlineGroups();
}

function refreshSelectionDependentUi() {
    releaseAddMinimalMode();
    applySystemFieldVisibility();
    refreshMediaDependentUi();
    enforceAddMinimalMode();
    refreshLanguageVariantLocks();
}

// System <-> media type filtering
function filterMediaTypes(forceDefault) {
    var sysSel = document.getElementById('system-select');
    var mediaSel = document.getElementById('media-select');
    if (!sysSel || !mediaSel || typeof SYSTEMS_MEDIA === 'undefined') return;
    var systemValue = sysSel.value || '';
    var currentVal = mediaSel.value;
    var opts = Array.prototype.slice.call(mediaSel.options);
    var byValue = {};
    var allowedLookup = {};
    for (var i = 0; i < opts.length; i++) {
        byValue[opts[i].value] = opts[i];
    }

    if (!systemValue) {
        if (byValue['']) {
            mediaSel.insertBefore(byValue[''], mediaSel.firstChild);
        }
        for (var blankIdx = 0; blankIdx < opts.length; blankIdx++) {
            opts[blankIdx].hidden = opts[blankIdx].value !== '';
        }
        mediaSel.value = '';
        return;
    }

    var allowed = SYSTEMS_MEDIA[systemValue] || [];
    if (byValue['']) {
        mediaSel.insertBefore(byValue[''], mediaSel.firstChild);
        byValue[''].hidden = true;
    }
    for (var j = 0; j < allowed.length; j++) {
        allowedLookup[allowed[j]] = true;
        if (byValue[allowed[j]]) {
            mediaSel.appendChild(byValue[allowed[j]]);
        }
    }
    for (var k = 0; k < opts.length; k++) {
        if (!allowedLookup[opts[k].value]) {
            mediaSel.appendChild(opts[k]);
        }
    }
    opts = mediaSel.options;
    for (var l = 0; l < opts.length; l++) {
        opts[l].hidden = opts[l].value === '' || allowed.indexOf(opts[l].value) === -1;
    }
    if ((forceDefault || currentVal === '' || allowed.indexOf(currentVal) === -1) && allowed.length > 0) {
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
    'barcode': true,
    'internal_serial': true,
    'volume_label': true,
    'universal_hash': true
};

var DEFAULT_WIDTHS = {
    'serial': 18,
    'edition': 18,
    'barcode': 18,
    'version': 6,
    'error_count': 6,
    'exe_date': 10,
    'internal_serial': 18,
    'volume_label': 18,
    'layerbreak': 8,
    'universal_hash': 18
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

function fitDiscMetaSelect(select) {
    if (!select) return;
    var font = getInputFont(select);
    var maxW = MIN_INPUT_WIDTH;
    Array.prototype.forEach.call(select.options || [], function (option) {
        if (option.hidden) return;
        var w = measureText(option.textContent || option.label || option.value, font) + 54;
        if (w > maxW) maxW = w;
    });
    select.style.width = maxW + 'px';
}

function fitDiscMetaFields() {
    document.querySelectorAll('.disc-meta-row > select').forEach(function (select) {
        fitDiscMetaSelect(select);
    });
    document.querySelectorAll('.disc-meta-row > input[name="disc_number"]').forEach(function (input) {
        var font = getInputFont(input);
        input.style.width = (measureText('000', font) + INPUT_PADDING) + 'px';
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
    'ring-mc': 40,
    'ring-ms': 8,
    'ring-tools': 5,
    'ring-moulds': 8,
    'ring-addmoulds': 10,
    'ring-offset': 3,
    'ring-offset-extra': 4,
    'ring-sample-start': 6,
    'ring-comment': 6
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

function layerbreakFromPicHex(hexText) {
    var bytes = [];
    var lines = hexText.split("\n");
    for (var li = 0; li < lines.length; li++) {
        var line = lines[li].trim();
        if (!line) continue;
        var hexPart = line.replace(/^[0-9A-Fa-f]{4}\s*:\s*/, "");
        var tokens = hexPart.trim().split(/\s+/);
        for (var ti = 0; ti < tokens.length; ti++) {
            if (!/^[0-9A-Fa-f]{2}$/.test(tokens[ti])) break;
            bytes.push(parseInt(tokens[ti], 16));
        }
    }
    if (bytes.length < 8) return null;
    var offset = 4;
    var cumulative = 0;
    var layerbreaks = [];
    while (offset + 0x40 <= bytes.length) {
        var block = bytes.slice(offset, offset + 0x40);
        if (block[0] !== 0x44 || block[1] !== 0x49) break; // 'DI'
        if (block[6] !== 0x20) return null; // not pressed BD
        var start = (block[24] << 24 | block[25] << 16 | block[26] << 8 | block[27]) >>> 0;
        var end   = (block[28] << 24 | block[29] << 16 | block[30] << 8 | block[31]) >>> 0;
        cumulative += end + 2 - start;
        layerbreaks.push(cumulative);
        offset += 0x40;
    }
    layerbreaks.pop(); // remove total size
    if (layerbreaks.length === 0) return null;
    return layerbreaks.map(function (v) { return v + ' (from PIC)'; }).join(', ');
}

function updatePicLayerbreakHint() {
    var hint = document.getElementById('pic-layerbreak-hint');
    var row = document.getElementById('layerbreak-row');
    if (!hint || !row || row.style.display === 'none') return;
    var picField = document.querySelector('[data-media-flag="has_pic"]');
    if (!picField || picField.style.display === 'none') { hint.style.display = 'none'; return; }
    var picTa = document.querySelector('textarea[name="pic"]');
    var result = picTa && picTa.value.trim() ? layerbreakFromPicHex(picTa.value) : null;
    if (result) {
        var values = result.split(', ').map(function (s) { return s.replace(' (from PIC)', ''); });
        var inputs = document.querySelectorAll('input[name="layerbreak"]');
        var allMatch = values.length === inputs.length && values.every(function (v, i) {
            return inputs[i] && inputs[i].value.trim() === v;
        });
        if (allMatch) {
            hint.style.display = 'none';
            return;
        }
        hint.textContent = result;
        hint.style.display = '';
    } else {
        hint.style.display = 'none';
    }
}

// Layerbreaks — fixed inputs based on media type layers
function renderLayerbreaks() {
    var row = document.getElementById('layerbreak-row');
    var container = document.getElementById('layerbreak-list');
    if (!row || !container) return;
    var ml = getMaxLayers();
    var mediaSel = document.getElementById('media-select');
    var hasDummy = mediaSel && typeof MEDIA_HAS_DUMMY_LAYER !== 'undefined' && !!MEDIA_HAS_DUMMY_LAYER[mediaSel.value];
    var count = hasDummy ? ml - 2 : ml - 1;
    if (count <= 0) {
        row.style.display = 'none';
        updatePicLayerbreakHint();
        return;
    }
    row.style.display = '';
    var hint = document.getElementById('pic-layerbreak-hint');
    container.innerHTML = '';
    for (var i = 0; i < count; i++) {
        var input = document.createElement('input');
        input.type = 'text';
        input.name = 'layerbreak';
        input.value = (typeof LAYERBREAKS !== 'undefined' && LAYERBREAKS[i]) ? LAYERBREAKS[i] : '';
        input.addEventListener('input', updatePicLayerbreakHint);
        container.appendChild(input);
    }
    if (hint) container.appendChild(hint);
    updatePicLayerbreakHint();
}

// Transliterate the Foreign Title into the target (Title) field via the API.
function setTransliterateNote(el, msg) {
    if (!el) return;
    el.textContent = msg || '';
    el.style.display = msg ? '' : 'none';
}

// Region (flag code) -> transliteration script. Only regions whose script we
// support show the Transliterate button.
var REGION_SCRIPT = { jp: 'japanese', ru: 'russian', gr: 'greek', cn: 'chinese', tw: 'chinese' };

// The script for the currently selected region(s), or null if none is supported.
function selectedTransliterationScript() {
    var inputs = document.querySelectorAll('input[name="regions"]');
    for (var i = 0; i < inputs.length; i++) {
        if (inputs[i].checked) {
            var script = REGION_SCRIPT[(inputs[i].dataset.flag || '').toLowerCase()];
            if (script) return script;
        }
    }
    return null;
}

function initTransliterate() {
    var btn = document.getElementById('transliterate-btn');
    if (!btn) return;
    var note = document.getElementById('transliterate-note');
    var foreign = document.querySelector('input[name="title_foreign"]');
    var target = document.querySelector('input[name="' + (btn.dataset.target || 'title') + '"]');
    if (!foreign || !target) {
        btn.style.display = 'none';
        return;
    }

    // Show the button only when the selected region has a supported script.
    function refreshVisibility() {
        btn.style.display = selectedTransliterationScript() ? '' : 'none';
    }
    refreshVisibility();
    document.querySelectorAll('input[name="regions"]').forEach(function (input) {
        input.addEventListener('change', refreshVisibility);
    });

    btn.addEventListener('click', function () {
        var script = selectedTransliterationScript();
        if (!script) {
            setTransliterateNote(note, 'Select a region with a supported script first.');
            return;
        }
        var text = foreign.value.trim();
        if (!text) {
            setTransliterateNote(note, 'Enter a Foreign Title first.');
            foreign.focus();
            return;
        }
        // Mirror the server limit (title_foreign is VARCHAR(512)).
        if (text.length > 512) {
            setTransliterateNote(note, 'Foreign Title is too long to transliterate (max 512 characters).');
            return;
        }
        // Don't silently clobber a title the user has already written.
        if (target.value.trim() && target.value.trim() !== text) {
            if (!window.confirm('Replace the current Title with the transliterated draft?')) {
                return;
            }
        }

        var original = btn.textContent;
        btn.disabled = true;
        btn.textContent = '…';
        setTransliterateNote(note, '');

        var csrfMeta = document.querySelector('meta[name="csrf-token"]');
        var csrfToken = csrfMeta ? csrfMeta.getAttribute('content') : '';

        fetch('/api/transliterate', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-CSRF-Token': csrfToken || ''
            },
            body: JSON.stringify({ text: text, script: script })
        }).then(function (resp) {
            if (!resp.ok) {
                return resp.text().then(function (t) {
                    throw new Error(t || ('HTTP ' + resp.status));
                });
            }
            return resp.json();
        }).then(function (data) {
            target.value = data.text || '';
            // Notify auto-resize / other listeners that the value changed.
            target.dispatchEvent(new Event('input', { bubbles: true }));
            target.focus();
            // Clear any prior message; we don't surface the draft notes in the UI.
            setTransliterateNote(note, '');
        }).catch(function (err) {
            setTransliterateNote(note, 'Could not transliterate: ' + (err && err.message ? err.message : 'error'));
        }).finally(function () {
            btn.disabled = false;
            btn.textContent = original;
        });
    });
}

var DUMP_LOG_SESSION_KEY = 'vgindex.dumpLogSession';
var pendingDumpLogFillMessage = '';

function getDumpLogFormIdentity(form) {
    var submissionToken = form.querySelector('[name="submission_token"]');
    return (submissionToken ? submissionToken.value : '') + '|' + form.action;
}

function saveDumpLogSession(form) {
    var textarea = form.querySelector('textarea[name="dump_log"]');
    var clearButton = form.querySelector('#dump-log-clear-btn');
    if (!textarea || !clearButton || clearButton.hidden) {
        clearDumpLogSession();
        return;
    }

    var saved = {
        state: textarea.classList.contains('dump-log-scrollable') ? 'uploaded' : 'filled',
        identity: getDumpLogFormIdentity(form),
        message: pendingDumpLogFillMessage
    };
    try {
        window.sessionStorage.setItem(DUMP_LOG_SESSION_KEY, JSON.stringify(saved));
    } catch (_error) {
        if (pendingDumpLogFillMessage) setDumpLogFillStatus(pendingDumpLogFillMessage);
    }
    pendingDumpLogFillMessage = '';
}

function clearDumpLogSession() {
    pendingDumpLogFillMessage = '';
    try {
        window.sessionStorage.removeItem(DUMP_LOG_SESSION_KEY);
    } catch (_error) {
        // Storage can be unavailable in privacy-restricted browser contexts.
    }
}

function setDumpLogControls(mode) {
    var uploadButton = document.getElementById('dump-log-upload-btn');
    var fillButton = document.getElementById('dump-log-fill-btn');
    var clearButton = document.getElementById('dump-log-clear-btn');
    var busy = mode === 'busy';
    var loaded = mode === 'loaded';
    if (uploadButton) {
        uploadButton.hidden = loaded;
        uploadButton.disabled = busy;
    }
    if (fillButton) {
        fillButton.hidden = loaded;
        fillButton.disabled = busy;
    }
    if (clearButton) clearButton.hidden = !loaded;
}

function setDumpLogTextareaState(textarea, uploaded) {
    if (!textarea) return;
    textarea.readOnly = uploaded;
    textarea.classList.toggle('dump-log-scrollable', uploaded);
    textarea.style.height = '';
}

function resetDumpLogInput(textarea) {
    textarea.value = '';
    textarea.dumpLogZipArchive = null;
    setDumpLogTextareaState(textarea, false);
    autoExpand(textarea);

    var fileInput = document.getElementById('dump-log-file-input');
    if (fileInput) fileInput.value = '';
}

function setDumpLogFillStatus(message) {
    var status = document.getElementById('dump-log-fill-status');
    if (!message) {
        if (status) status.remove();
        return;
    }
    if (!status) {
        status = document.createElement('div');
        status.id = 'dump-log-fill-status';
        status.className = 'validation-result validation-result-new';
        status.setAttribute('role', 'status');
        var anchor = document.querySelector('.validation-result, .validation-errors, #disc-edit-form');
        if (!anchor || !anchor.parentNode) return;
        anchor.parentNode.insertBefore(status, anchor);
    }
    status.textContent = message;
}

function preserveDumpLogFillStatus(message) {
    pendingDumpLogFillMessage = message;
}

function restoreDumpLogFillStatus() {
    var saved = null;
    try {
        var stored = window.sessionStorage.getItem(DUMP_LOG_SESSION_KEY);
        window.sessionStorage.removeItem(DUMP_LOG_SESSION_KEY);
        saved = stored ? JSON.parse(stored) : null;
    } catch (_error) {
        // Storage can be unavailable in privacy-restricted browser contexts.
    }

    var form = document.getElementById('disc-edit-form');
    var textarea = form ? form.querySelector('textarea[name="dump_log"]') : null;
    var matchesForm = saved
        && form
        && textarea
        && saved.identity === getDumpLogFormIdentity(form);
    if (!matchesForm) return;

    if (saved.state === 'uploaded') {
        setDumpLogTextareaState(textarea, true);
    }
    if (saved.state === 'uploaded' || saved.state === 'filled') {
        setDumpLogControls('loaded');
    }
    if (!saved.message) return;

    setDumpLogFillStatus(saved.message);

    var validationErrors = document.querySelector('.validation-errors');
    if (validationErrors) {
        validationErrors.classList.add('dump-log-fill');
        var strong = validationErrors.querySelector('strong:first-child');
        if (strong) {
            strong.textContent = 'Please fill in these required fields:';
        }
    }
}

function applyParsedField(name, value) {
    if (value === null || value === undefined) return 0;
    var input = document.querySelector('[name="' + name + '"]');
    if (!input) return 0;
    input.value = String(value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
    fitInlineGroupForInput(input);
    return 1;
}

function applyParsedRedumperLog(data) {
    var applied = 0;
    saveRingFromDom();
    var systemSelect = document.getElementById('system-select');
    if (data.system_code !== null && data.system_code !== undefined && systemSelect) {
        var optionExists = Array.prototype.some.call(systemSelect.options, function (option) {
            return option.value === data.system_code;
        });
        if (optionExists) {
            systemSelect.value = data.system_code;
            systemSelect.dispatchEvent(new Event('change', { bubbles: true }));
            applied += 1;
        }
    }
    applied += applyParsedField('media_type', data.media_type);

    [
        'version',
        'exe_date',
        'error_count',
        'internal_serial',
        'volume_label',
        'universal_hash',
        'sector_ranges',
        'sbi',
        'pvd',
        'header',
        'bca',
        'cuesheet',
        'dat',
        'extras',
        'protection'
    ].forEach(function (name) {
        applied += applyParsedField(name, data[name]);
    });

    if (data.edc !== null && data.edc !== undefined) {
        var edc = document.querySelector('input[name="edc"][value="' + (data.edc ? 'true' : 'false') + '"]');
        if (edc) {
            edc.checked = true;
            edc.dispatchEvent(new Event('change', { bubbles: true }));
            applied += 1;
        }
    }

    if ((data.offset_value !== null && data.offset_value !== undefined)
        || (data.sample_start !== null && data.sample_start !== undefined)) {
        saveRingFromDom();
        ensureEmptyRingEntry();
        if (ringEntries.length > 0) {
            if (data.offset_value !== null && data.offset_value !== undefined) {
                ringEntries[0].offset_value = String(data.offset_value);
                applied += 1;
            }
            if (data.sample_start !== null && data.sample_start !== undefined) {
                ringEntries[0].sample_start = String(data.sample_start);
                applied += 1;
            }
            renderRingEntries();
            fitRingColumns();
        }
    }

    if (Array.isArray(data.layerbreaks) && data.layerbreaks.length > 0) {
        var lbInputs = document.querySelectorAll('input[name="layerbreak"]');
        for (var lbi = 0; lbi < lbInputs.length && lbi < data.layerbreaks.length; lbi++) {
            lbInputs[lbi].value = String(data.layerbreaks[lbi]);
            LAYERBREAKS[lbi] = lbInputs[lbi].value;
            lbInputs[lbi].dispatchEvent(new Event('input', { bubbles: true }));
            applied += 1;
        }
        fitAllInlineGroups();
    }

    fitDiscMetaFields();
    fitAllInlineGroups();
    return applied;
}

function enableAddMinimalFieldsForSubmit(form) {
    form.querySelectorAll(
        '[data-add-requires-system-media] input:disabled, '
        + '[data-add-requires-system-media] select:disabled, '
        + '[data-add-requires-system-media] textarea:disabled'
    ).forEach(function (control) {
        control.disabled = false;
    });
}

var MAX_DUMP_LOG_ZIP_SIZE = 10 * 1024 * 1024;
var MAX_DUMP_LOG_FILE_SIZE = 3 * 1024 * 1024;

function hasFileExtension(file, extension) {
    return file.name.toLowerCase().endsWith(extension);
}

function findZipFiles(bytes, matchesFile) {
    var matches = [];
    fflate.unzipSync(bytes, {
        filter: function (file) {
            if (matchesFile(file)) matches.push(file);
            return false;
        }
    });
    return matches;
}

function extractZipFiles(bytes, files) {
    var names = Object.create(null);
    files.forEach(function (file) {
        names[file.name] = true;
    });
    return fflate.unzipSync(bytes, {
        filter: function (file) {
            return !!names[file.name];
        }
    });
}

function applyDumpLogJsonField(name, value, replace) {
    var form = document.getElementById('disc-edit-form');
    var controls = form ? Array.prototype.filter.call(
        form.querySelectorAll('[name], [data-json-seed-key]'),
        function (control) {
            return control.name === name || control.getAttribute('data-json-seed-key') === name;
        }
    ) : [];
    if (!controls.length || controls[0].hasAttribute('data-no-json-seed')) return 0;

    if (Array.isArray(value)) {
        if (controls[0].type === 'checkbox') {
            if (!value.length || controls.some(function (item) { return item.checked; })) return 0;
            return value.reduce(function (applied, item) {
                return applied + applyDumpLogJsonScalar(item, true, controls);
            }, 0);
        }
        if (value.length !== 1) return 0;
        value = value[0];
    }

    return applyDumpLogJsonScalar(value, replace, controls);
}

function applyDumpLogJsonScalar(value, replace, controls) {
    if (value === null || value === undefined || typeof value === 'object'
        || String(value).trim() === '') return 0;

    var isChoice = controls[0].type === 'radio' || controls[0].type === 'checkbox';
    if (!replace && controls.some(function (item) {
        return isChoice ? item.checked : (item.value || '').trim() !== '';
    })) {
        return 0;
    }

    var text = String(value);
    var control = isChoice
        ? controls.find(function (item) { return item.value === text; })
        : controls[0];
    if (!control || control.checked) return 0;
    if (isChoice) {
        control.checked = true;
    } else {
        if (control.tagName === 'SELECT'
            && !Array.prototype.some.call(control.options, function (option) {
                return option.value === text && !option.hidden && !option.disabled;
            })) {
            return 0;
        }
        control.value = text;
    }
    control.dispatchEvent(new Event('input', { bubbles: true }));
    control.dispatchEvent(new Event('change', { bubbles: true }));
    fitInlineGroupForInput(control);
    return 1;
}

function collectDumpLogJsonFields(data, valuesByName) {
    if (!data || typeof data !== 'object' || Array.isArray(data)) return;
    Object.keys(data).forEach(function (name) {
        var value = data[name];
        if (value && typeof value === 'object' && !Array.isArray(value)) {
            collectDumpLogJsonFields(value, valuesByName);
        } else if (Array.isArray(value) && value.length === 1
            && value[0] && typeof value[0] === 'object') {
            collectDumpLogJsonFields(value[0], valuesByName);
        } else {
            if (!valuesByName[name]) valuesByName[name] = [];
            valuesByName[name].push(value);
        }
    });
}

function parseDumpLogZipJson(bytes) {
    try {
        var files = findZipFiles(bytes, function (file) {
            return hasFileExtension(file, '.json');
        });
        if (files.length !== 1 || files[0].originalSize > MAX_DUMP_LOG_FILE_SIZE) return null;
        var extracted = extractZipFiles(bytes, files);
        return JSON.parse(fflate.strFromU8(extracted[files[0].name]));
    } catch (_error) {
        return null;
    }
}

function applyDumpLogJsonText(valuesByName) {
    var textarea = document.querySelector('textarea[name="dump_log"]');
    if (!textarea) return 0;
    var values = valuesByName.dump_log || [];
    var value = values.length ? values[0] : null;
    if (value === null || value === undefined) {
        values = valuesByName.dumping_program || [];
        value = values.length ? values[0] : null;
    }
    if (Array.isArray(value)) {
        if (value.length !== 1) return 0;
        value = value[0];
    }
    if (value === null || value === undefined || typeof value === 'object'
        || String(value).trim() === '') return 0;

    textarea.dumpLogZipArchive = null;
    textarea.value = String(value);
    setDumpLogTextareaState(textarea, false);
    setDumpLogControls('ready');
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
    return 1;
}

function applyDumpLogZipJson(data, pasteDumpLog) {
    try {
        var valuesByName = Object.create(null);
        collectDumpLogJsonFields(data, valuesByName);
        var applied = 0;
        ['system_code', 'media_type'].forEach(function (name) {
            (valuesByName[name] || []).forEach(function (value) {
                applied += applyDumpLogJsonField(name, value, true);
            });
        });
        if (pasteDumpLog) {
            applied += applyDumpLogJsonText(valuesByName);
        }
        var form = document.getElementById('disc-edit-form');
        var visited = Object.create(null);
        if (form) form.querySelectorAll('[name], [data-json-seed-key]').forEach(function (control) {
            var name = control.getAttribute('data-json-seed-key') || control.name;
            if (!name || name === 'system_code' || name === 'media_type'
                || name === 'dump_log' || name === 'dumping_program' || visited[name]) return;
            visited[name] = true;
            (valuesByName[name] || []).forEach(function (value) {
                applied += applyDumpLogJsonField(name, value);
            });
        });
        return applied;
    } catch (_error) {
        return 0;
    }
}

function applyDumpLogZipFields(bytes) {
    var fields = [
        { name: 'dmi', extension: '.manufacturer', size: 2052 },
        { name: 'pic', extension: '.physical', size: 4100 },
        { name: 'bca', extension: '.bca', minSize: 16, maxSize: 192 }
    ];
    try {
        var entries = findZipFiles(bytes, function (file) {
            return fields.some(function (field) {
                return hasFileExtension(file, field.extension);
            });
        });
        var selected = [];
        fields.forEach(function (field) {
            var control = document.querySelector('[name="' + field.name + '"]');
            var matches = entries.filter(function (file) {
                return hasFileExtension(file, field.extension);
            });
            if (!control || control.disabled || matches.length !== 1) return;
            var file = matches[0];
            if ((field.size !== undefined && file.originalSize !== field.size)
                || (field.minSize !== undefined && file.originalSize < field.minSize)
                || (field.maxSize !== undefined && file.originalSize > field.maxSize)) {
                return;
            }
            selected.push({ name: field.name, file: file });
        });

        var applied = 0;
        selected.forEach(function (item) {
            try {
                var control = document.querySelector('[name="' + item.name + '"]');
                if (!control || control.disabled) return;
                var extracted = extractZipFiles(bytes, [item.file]);
                var data = extracted[item.file.name];
                if (!data) return;
                var hex = Array.prototype.map.call(data, function (byte) {
                    return byte.toString(16).padStart(2, '0').toUpperCase();
                }).join(' ');
                applied += applyParsedField(item.name, hex);
            } catch (_error) {
                // A malformed optional sidecar should not block the other fields.
            }
        });
        return applied;
    } catch (_error) {
        return 0;
    }
}

function initDumpLogDragDrop() {
    var textarea = document.querySelector('textarea[name="dump_log"]');
    var fillButton = document.getElementById('dump-log-fill-btn');
    var uploadButton = document.getElementById('dump-log-upload-btn');
    var fileInput = document.getElementById('dump-log-file-input');
    if (!textarea || !fillButton) return;

    if (!textarea.dumpLogLatestUploadId) {
        textarea.dumpLogLatestUploadId = 0;
    }

    function rejectUploadWhileParsing() {
        if (!fillButton.disabled) return false;
        setDumpLogFillStatus('Wait for the current log to finish parsing before uploading another.');
        return true;
    }

    function useUploadedLog(text, zipArchive, uploadId) {
        if (uploadId !== textarea.dumpLogLatestUploadId || rejectUploadWhileParsing()) return;
        textarea.value = text;
        textarea.dumpLogZipArchive = zipArchive;
        setDumpLogTextareaState(textarea, true);
        fillButton.click();
    }

    function loadDumpLogFile(file) {
        // Validate before parsing ZIP
        if (!file || rejectUploadWhileParsing()) return;
        var uploadId = ++textarea.dumpLogLatestUploadId;
        var extension = file.name.toLowerCase().split('.').pop();
        if (extension !== 'log' && extension !== 'zip') {
            setDumpLogFillStatus('Only redumper .log and .zip files are supported.');
            return;
        }
        if (extension === 'zip' && file.size >= MAX_DUMP_LOG_ZIP_SIZE) {
            setDumpLogFillStatus('ZIP is too large to safely extract in browser!');
            return;
        }
        if (extension === 'log' && file.size > MAX_DUMP_LOG_FILE_SIZE) {
            setDumpLogFillStatus('Log file is too large.');
            return;
        }

        if (extension === 'zip') {
            file.arrayBuffer().then(function (buffer) {
                if (uploadId !== textarea.dumpLogLatestUploadId || rejectUploadWhileParsing()) return;
                var bytes = new Uint8Array(buffer);
                var logs = findZipFiles(bytes, function (file) {
                    return hasFileExtension(file, '.log');
                });
                if (logs.length > 1) {
                    setDumpLogFillStatus('ZIP files may contain at most one .log file.');
                    return;
                }
                if (logs.length === 0) {
                    var seed = parseDumpLogZipJson(bytes);
                    if (!seed || typeof seed !== 'object' || Array.isArray(seed)) {
                        setDumpLogFillStatus('Log ZIP without a Redumper .log must contain a seed .json file.');
                        return;
                    }
                    textarea.dumpLogZipArchive = null;
                    setDumpLogTextareaState(textarea, false);
                    setDumpLogControls('ready');
                    var seeded = applyDumpLogZipJson(seed, true);
                    seeded += applyDumpLogZipFields(bytes);
                    setDumpLogFillStatus(seeded
                        ? 'Filled ' + seeded + ' field(s) from the ZIP. Redumper Log parsing was not run.'
                        : 'No supported values were found in the seed JSON.');
                    return;
                }
                if (logs[0].originalSize > MAX_DUMP_LOG_FILE_SIZE) {
                    setDumpLogFillStatus('The log file inside the ZIP is too large.');
                    return;
                }
                var logFiles = extractZipFiles(bytes, logs);
                var logBytes = logFiles[logs[0].name];
                if (!logBytes) throw new Error('Could not extract the log file from the ZIP.');
                useUploadedLog(
                    fflate.strFromU8(logBytes),
                    { bytes: bytes },
                    uploadId
                );
            }).catch(function (error) {
                if (uploadId === textarea.dumpLogLatestUploadId) {
                    setDumpLogFillStatus(error && error.message
                        ? error.message
                        : 'Could not read the ZIP file.');
                }
            });
            return;
        }

        var reader = new FileReader();
        reader.onload = function (ev) {
            if (uploadId !== textarea.dumpLogLatestUploadId || rejectUploadWhileParsing()) return;
            useUploadedLog(ev.target.result, null, uploadId);
        };
        reader.onerror = function () {
            if (uploadId === textarea.dumpLogLatestUploadId) {
                setDumpLogFillStatus('Failed to read the selected log file.');
            }
        };
        reader.readAsText(file);
    }

    if (uploadButton && fileInput) {
        uploadButton.addEventListener('click', function () {
            fileInput.click();
        });
        fileInput.addEventListener('change', function () {
            loadDumpLogFile(fileInput.files[0]);
            fileInput.value = '';
        });
    }

    textarea.addEventListener('dragover', function (e) {
        e.preventDefault();
        e.dataTransfer.dropEffect = 'copy';
    });

    textarea.addEventListener('drop', function (e) {
        e.preventDefault();
        loadDumpLogFile(e.dataTransfer.files[0]);
    });
}

function initDumpLogFill() {
    var button = document.getElementById('dump-log-fill-btn');
    var clearButton = document.getElementById('dump-log-clear-btn');
    var textarea = document.querySelector('textarea[name="dump_log"]');
    if (!button || !textarea) return;

    textarea.addEventListener('input', function () {
        textarea.dumpLogZipArchive = null;
        if (!clearButton || clearButton.hidden || textarea.readOnly) return;
        setDumpLogControls('ready');
        clearDumpLogSession();
    });

    if (clearButton) {
        clearButton.addEventListener('click', function () {
            resetDumpLogInput(textarea);
            textarea.dumpLogLatestUploadId++;
            setDumpLogFillStatus('');
            setDumpLogControls('ready');
            clearDumpLogSession();
            textarea.focus();
        });
    }

    button.addEventListener('click', function () {
        if (!textarea.value.trim()) {
            setDumpLogFillStatus('Paste a redumper log first.');
            textarea.focus();
            return;
        }
        var zipArchive = textarea.dumpLogZipArchive || null;
        textarea.dumpLogZipArchive = null;
        var requestBody = JSON.stringify({ log: textarea.value });
        setDumpLogControls('busy');

        var csrfMeta = document.querySelector('meta[name="csrf-token"]');
        var csrfToken = csrfMeta ? csrfMeta.getAttribute('content') : '';
        setDumpLogFillStatus('Reading log…');

        fetch('/api/parse-redumper-log', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-CSRF-Token': csrfToken || ''
            },
            body: requestBody
        }).then(function (response) {
            if (response.status === 422) {
                return response.json().then(function (data) {
                    var error = new Error('Unsupported redumper build');
                    if (data
                        && data.error === 'unsupported_redumper_build'
                        && typeof data.message === 'string') {
                        error.dumpLogFillMessage = data.message;
                    }
                    error.invalidDumpLog = true;
                    throw error;
                });
            }
            if (!response.ok) {
                throw new Error('HTTP ' + response.status);
            }
            return response.json();
        }).then(function (data) {
            var applied = applyParsedRedumperLog(data);
            var warningMessage = '';
            if (zipArchive) {
                var seed = parseDumpLogZipJson(zipArchive.bytes);
                if (seed && typeof seed === 'object' && !Array.isArray(seed)) {
                    // Warn if seed JSON differs from redumper log parsing
                    if (data.system_code && String(data.system_code).trim()) {
                        var seedValues = Object.create(null);
                        collectDumpLogJsonFields(seed, seedValues);
                        var seedSystem = seedValues.system_code;
                        var seedMedia = seedValues.media_type;
                        var seedSystemValue = seedSystem && seedSystem[seedSystem.length - 1];
                        var seedMediaValue = seedMedia && seedMedia[seedMedia.length - 1];
                        var differs = (seedSystemValue
                            && String(seedSystemValue) !== String(data.system_code))
                            || (data.media_type && seedMediaValue
                                && String(seedMediaValue) !== String(data.media_type));
                        if (differs) {
                            warningMessage = ' Warning: Set to ' + (seedSystemValue || data.system_code) + ' ' + (seedMediaValue || data.media_type || '')
                                + ' but Redumper log suggests ' + data.system_code + ' ' + (data.media_type || '');
                        }
                    }
                    applied += applyDumpLogZipJson(seed, false);
                }
                applied += applyDumpLogZipFields(zipArchive.bytes);
            }
            if (applied === 0) {
                if (textarea.classList.contains('dump-log-scrollable')) {
                    setDumpLogControls('loaded');
                } else {
                    setDumpLogControls('ready');
                }
                setDumpLogFillStatus('No supported values were found.' + warningMessage);
                return;
            }

            setDumpLogControls('loaded');
            var message = 'Filled ' + applied + (applied === 1 ? ' field.' : ' fields.') + warningMessage;
            var form = document.getElementById('disc-edit-form');
            var validateButton = form
                ? form.querySelector('button[type="submit"][name="action"][value="validate"]')
                : null;
            if (!form || !validateButton) {
                setDumpLogFillStatus(message);
                return;
            }
            var systemSelect = document.getElementById('system-select');
            if (!systemSelect || !(systemSelect.value || '').trim()) {
                enableAddMinimalFieldsForSubmit(form);
            }
            preserveDumpLogFillStatus(message);
            form.requestSubmit(validateButton);
        }).catch(function (error) {
            if (error && error.invalidDumpLog && textarea.classList.contains('dump-log-scrollable')) {
                resetDumpLogInput(textarea);
            } else {
                setDumpLogTextareaState(textarea, false);
            }
            setDumpLogControls('ready');
            setDumpLogFillStatus(error && error.dumpLogFillMessage
                ? error.dumpLogFillMessage
                : 'Could not read the log. Please try again.');
        });
    });
}

// Init
document.addEventListener('DOMContentLoaded', function () {
    filterMediaTypes();
    refreshSelectionDependentUi();
    initRingEditor();
    renderLayerbreaks();
    enforceAddMinimalMode();
    initAutoExpand();
    restoreDumpLogFillStatus();
    initEditionSelectors();
    initSubmitAsSelector();
    initFlagListToggle();
    initLanguageVariantSelection();
    initTransliterate();
    initDumpLogFill();
    initDumpLogDragDrop();
    initIndependentInlineResizing();
    var picTa = document.querySelector('textarea[name="pic"]');
    if (picTa) picTa.addEventListener('input', updatePicLayerbreakHint);
    fitDiscMetaFields();
    fitAllInlineGroups();
    fitRingColumns();

    var sysSel = document.getElementById('system-select');
    if (sysSel) {
        sysSel.addEventListener('change', function () {
            filterMediaTypes(true);
            refreshEditionSelectors();
            refreshSelectionDependentUi();
            fitDiscMetaFields();
        });
    }
    var mediaSel = document.getElementById('media-select');
    if (mediaSel) {
        mediaSel.addEventListener('change', function () {
            refreshSelectionDependentUi();
            fitDiscMetaFields();
        });
    }

    var form = document.getElementById('disc-edit-form');
    if (form) {
        form.addEventListener('submit', function (event) {
            if (form.dataset.submitting === 'true') {
                event.preventDefault();
                return;
            }

            var data = collectRingCodes();
            document.getElementById('ring-codes-json').value = JSON.stringify(data);

            var submitter = event.submitter;
            if (submitter
                && submitter.name === 'action'
                && (submitter.value === 'validate' || submitter.value === 'save')) {
                saveDumpLogSession(form);
            }
            if (submitter && submitter.name) {
                var preservedAction = document.createElement('input');
                preservedAction.type = 'hidden';
                preservedAction.name = submitter.name;
                preservedAction.value = submitter.value;
                preservedAction.dataset.preservedSubmitAction = 'true';
                form.appendChild(preservedAction);
            }

            form.dataset.submitting = 'true';
            form.setAttribute('aria-busy', 'true');
            form.querySelectorAll('button[type="submit"], input[type="submit"]').forEach(function (button) {
                button.disabled = true;
            });
        });

        window.addEventListener('pageshow', function (event) {
            if (!event.persisted) return;
            delete form.dataset.submitting;
            form.removeAttribute('aria-busy');
            form.querySelectorAll('button[type="submit"], input[type="submit"]').forEach(function (button) {
                button.disabled = false;
            });
            form.querySelectorAll('[data-preserved-submit-action="true"]').forEach(function (input) {
                input.remove();
            });
        });
    }
});
