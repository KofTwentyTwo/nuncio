import React, { useState } from 'react';
import { Folder } from '../types';

interface SidebarProps {
  folders: Folder[];
  activeFolderId: string;
  onSelectFolder: (folderId: string) => void;
  status: string;
  onSync: () => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  folders,
  activeFolderId,
  onSelectFolder,
  status,
  onSync,
}) => {
  const [showLicenses, setShowLicenses] = useState(false);

  return (
    <aside className="sidebar">
      <div className="brand-header">
        <img src="/assets/logo.jpg" alt="Nuncio Logo" className="brand-logo-img" style={{ width: '28px', height: '28px', borderRadius: '6px', objectFit: 'cover' }} />
        <h1 className="brand-title">Nuncio</h1>
      </div>

      <nav className="nav-section">
        <div className="nav-title">Mailboxes</div>
        {folders.map((folder) => (
          <div
            key={folder.id}
            className={`nav-item ${activeFolderId === folder.id ? 'active' : ''}`}
            onClick={() => onSelectFolder(folder.id)}
          >
            <div className="nav-item-left">
              <span>{folder.name}</span>
            </div>
            {folder.unreadCount > 0 && (
              <span className="badge">{folder.unreadCount}</span>
            )}
          </div>
        ))}
      </nav>

      <div className="sidebar-footer">
        <div className="status-indicator">
          <div className={`status-dot ${status === 'Syncing' ? 'syncing' : ''}`} />
          <span>{status}</span>
        </div>
        <button className="btn btn-primary" onClick={onSync}>
          Sync
        </button>
        <button
          className="btn"
          style={{ width: '100%', marginTop: '6px', fontSize: '11px', background: 'rgba(255, 255, 255, 0.05)', color: '#a0aec0', border: '1px solid rgba(255, 255, 255, 0.1)' }}
          onClick={() => setShowLicenses(true)}
        >
          Licenses & Open Source Credits
        </button>
      </div>

      {showLicenses && (
        <div style={{
          position: 'fixed',
          top: 0,
          left: 0,
          right: 0,
          bottom: 0,
          background: 'rgba(0, 0, 0, 0.8)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          zIndex: 999,
        }}>
          <div style={{
            background: '#1a202c',
            border: '1px solid #4a5568',
            borderRadius: '12px',
            padding: '24px',
            maxWidth: '520px',
            width: '90%',
            color: '#e2e8f0',
          }}>
            <h2 style={{ marginTop: 0, color: '#63b3ed' }}>Third-Party Open Source Credits</h2>
            <p style={{ fontSize: '13px', color: '#a0aec0' }}>Nuncio is built on the following open source libraries:</p>
            <ul style={{ fontSize: '12px', paddingLeft: '20px', lineHeight: '1.6' }}>
              <li><strong>Tokio</strong> (MIT) — Asynchronous I/O Runtime</li>
              <li><strong>Ratatui</strong> (MIT) — Terminal UI Rendering Engine</li>
              <li><strong>Tauri v2</strong> (MIT/Apache-2.0) — Lightweight WebView Shell</li>
              <li><strong>SQLx</strong> (MIT/Apache-2.0) — Async SQLite Driver</li>
              <li><strong>Lettre</strong> (MIT) — SMTP Transport Client</li>
              <li><strong>async-imap</strong> (MIT/Apache-2.0) — Async IMAP Client</li>
              <li><strong>AES-256-GCM / age</strong> (MIT/Apache-2.0) — Zero-Trust Column & Stream Encryption</li>
              <li><strong>Zeroize / Keyring</strong> (MIT/Apache-2.0) — Secure Memory Wiping & OS Keyring Integration</li>
            </ul>
            <div style={{ textAlign: 'right', marginTop: '16px' }}>
              <button className="btn btn-primary" onClick={() => setShowLicenses(false)}>Close</button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
};
