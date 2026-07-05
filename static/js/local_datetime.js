(function () {
    var formatOptions = {
        date: { dateStyle: 'medium' },
        minute: { dateStyle: 'medium', timeStyle: 'short' },
        second: { dateStyle: 'medium', timeStyle: 'medium' }
    };

    function localizeTimestamp(element) {
        var options = formatOptions[element.getAttribute('data-local-datetime')];
        if (!options) return;

        var date = new Date(element.getAttribute('datetime'));
        if (isNaN(date.getTime())) return;

        element.textContent = new Intl.DateTimeFormat(undefined, options).format(date);
    }

    function localizeTimestamps(root) {
        if (root.matches && root.matches('[data-local-datetime]')) {
            localizeTimestamp(root);
        }
        root.querySelectorAll('[data-local-datetime]').forEach(localizeTimestamp);
    }

    document.addEventListener('DOMContentLoaded', function () {
        localizeTimestamps(document);
    });
    document.addEventListener('htmx:load', function (event) {
        localizeTimestamps(event.detail.elt);
    });
})();
