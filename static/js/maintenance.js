function maintenanceRowPayload(row) {
    var payload = {
        original_code: row.dataset.originalCode || ''
    };
    row.querySelectorAll('[data-maintenance-field]').forEach(function (input) {
        payload[input.getAttribute('data-maintenance-field') || ''] = input.value;
    });

    var flags = Array.prototype.map.call(
        row.querySelectorAll('[data-maintenance-flag]:checked'),
        function (input) { return input.getAttribute('data-maintenance-flag') || ''; }
    );
    if (flags.length > 0 || row.querySelector('[data-maintenance-flag]')) {
        payload.flags = flags;
    }
    return payload;
}

function serializeMaintenanceForm(form) {
    var payloadSelector = form.getAttribute('data-maintenance-payload');
    var payload = payloadSelector ? form.querySelector(payloadSelector) : null;
    if (!payload) return;
    payload.value = JSON.stringify({
        rows: Array.prototype.map.call(
            form.querySelectorAll('[data-maintenance-row]'),
            maintenanceRowPayload
        )
    });
}

function updateFlagPreview(select) {
    var row = select.closest('[data-maintenance-row]');
    var preview = row ? row.querySelector('[data-flag-preview]') : null;
    if (!preview) return;

    if (select.value) {
        preview.src = '/static/flags/' + select.value + '.svg';
        preview.alt = select.value;
        preview.title = select.value;
        preview.hidden = false;
    } else {
        preview.src = 'about:blank';
        preview.alt = '';
        preview.title = '';
        preview.hidden = true;
    }
}

document.addEventListener('DOMContentLoaded', function () {
    document.querySelectorAll('[data-maintenance-form]').forEach(function (form) {
        form.addEventListener('submit', function () {
            serializeMaintenanceForm(form);
        });
    });

    document.querySelectorAll('[data-flag-select]').forEach(function (select) {
        updateFlagPreview(select);
        select.addEventListener('change', function () {
            updateFlagPreview(select);
        });
    });
});
