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

function initMaintenanceUserPicker() {
    var input = document.getElementById('maintenance-user-input');
    var select = document.getElementById('maintenance-user-select');
    var actions = document.getElementById('maintenance-user-actions');
    var selectedUsername = document.getElementById('maintenance-selected-username');
    var selectedSessions = document.getElementById('maintenance-selected-sessions');
    var renameForm = document.getElementById('maintenance-user-rename-form');
    var renameInput = document.getElementById('maintenance-new-username');
    var renameButton = document.getElementById('maintenance-user-rename-button');
    var logoutForm = document.getElementById('maintenance-user-logout-form');
    var logoutButton = document.getElementById('maintenance-user-logout-button');
    var deleteForm = document.getElementById('maintenance-user-delete-form');
    var deleteConfirmation = document.getElementById('maintenance-delete-confirmation');
    var deleteButton = document.getElementById('maintenance-user-delete-button');
    if (!input || !select || !actions || !renameForm || !logoutForm || !deleteForm || !deleteConfirmation || !deleteButton) return;

    var usersByName = Object.create(null);
    var selectedDeleteUsername = '';
    Array.prototype.forEach.call(select.options, function (option) {
        var username = option.getAttribute('data-username');
        if (username) usersByName[username] = option;
    });

    function clearSelection() {
        actions.hidden = true;
        renameForm.action = '/maintenance';
        logoutForm.action = '/maintenance';
        deleteForm.action = '/maintenance';
        renameButton.disabled = true;
        logoutButton.disabled = true;
        selectedDeleteUsername = '';
        deleteConfirmation.value = '';
        deleteConfirmation.disabled = true;
        deleteButton.disabled = true;
    }

    function updateDeleteButton() {
        deleteButton.disabled = selectedDeleteUsername === '' ||
            selectedDeleteUsername === 'Deleted' ||
            deleteConfirmation.value !== selectedDeleteUsername;
    }

    function selectUser(option) {
        if (!option || !option.value) {
            clearSelection();
            return;
        }

        var username = option.getAttribute('data-username') || '';
        var sessions = Number(option.getAttribute('data-session-count') || '0');
        selectedUsername.textContent = username;
        selectedSessions.textContent = sessions + (sessions === 1 ? ' app session' : ' app sessions');
        renameInput.value = username;
        renameForm.action = '/maintenance/users/' + option.value + '/rename';
        logoutForm.action = '/maintenance/users/' + option.value + '/logout';
        deleteForm.action = '/maintenance/users/' + option.value + '/delete';
        renameButton.disabled = false;
        logoutButton.disabled = false;
        selectedDeleteUsername = username;
        deleteConfirmation.value = '';
        deleteConfirmation.disabled = username === 'Deleted';
        updateDeleteButton();
        actions.hidden = false;
    }

    select.addEventListener('change', function () {
        var option = select.options[select.selectedIndex];
        if (!option || !option.value) return;
        input.value = option.getAttribute('data-username') || '';
        selectUser(option);
        select.value = '';
        input.focus();
    });

    input.addEventListener('input', function () {
        selectUser(usersByName[input.value]);
    });

    deleteConfirmation.addEventListener('input', updateDeleteButton);

    clearSelection();
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

    initMaintenanceUserPicker();
});
