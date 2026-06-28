export default function MailHeadersPanel({
  text,
  loading = false,
  error = '',
  copied = false,
  onCopy,
  onDownload,
  onClose,
}) {
  return (
    <div className="db-mail-headers-overlay" role="presentation" onMouseDown={onClose}>
      <section
        className="db-mail-headers-panel"
        aria-label="Raw message source"
        role="dialog"
        aria-modal="true"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="db-mail-headers-panel__bar">
          <div className="db-mail-headers-panel__title">Message source</div>
          <div className="db-mail-headers-panel__actions">
            <button type="button" onClick={onCopy} disabled={loading || !text}>
              {copied ? 'Copied' : 'Copy'}
            </button>
            <button type="button" onClick={onDownload} disabled={loading || !text}>
              Download
            </button>
            <button type="button" onClick={onClose} aria-label="Close message source">
              <img src="/img/icons/close.svg" className="svg-icon-inline" />
            </button>
          </div>
        </div>
        {loading ? (
          <div className="db-mail-headers-panel__status">Loading message source...</div>
        ) : error ? (
          <div className="db-mail-headers-panel__status error">{error}</div>
        ) : (
          <pre className="db-mail-headers-panel__text">{text || 'No message source available.'}</pre>
        )}
      </section>
    </div>
  )
}
