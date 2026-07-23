import React from 'react';
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
      </div>
    </aside>
  );
};
