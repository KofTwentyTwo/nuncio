import { useState, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Sidebar } from './components/Sidebar';
import { MessageList } from './components/MessageList';
import { Reader } from './components/Reader';
import { Folder, Message } from './types';

const INITIAL_FOLDERS: Folder[] = [
  { id: 'inbox', name: 'Inbox', unreadCount: 2 },
  { id: 'sent', name: 'Sent', unreadCount: 0 },
  { id: 'archive', name: 'Archive', unreadCount: 0 },
  { id: 'trash', name: 'Trash', unreadCount: 0 },
];

const INITIAL_MESSAGES: Message[] = [
  {
    id: 'msg-1',
    sender: 'Stalwart Labs',
    senderEmail: 'releases@stalw.art',
    subject: 'JMAP Engine v0.9 Released: Zero-copy MIME parser',
    snippet: 'We are thrilled to announce the release ofstalwart-JMAP with high-performance parsing...',
    bodyHtml: `
      <div style="font-family: sans-serif; color: #1e293b; padding: 16px;">
        <h2>JMAP Engine v0.9 Released</h2>
        <p>Hello Nuncio Team,</p>
        <p>The high-performance zero-copy MIME parser using SIMD Base64 decoding is now integrated into the core engine architecture.</p>
        <p>Best regards,<br/>Stalwart Labs</p>
      </div>
    `,
    date: '10:42 AM',
    read: false,
    folderId: 'inbox',
  },
  {
    id: 'msg-2',
    sender: 'CalDAV Protocol Service',
    senderEmail: 'sync@nuncio.mx',
    subject: 'Calendar Sync completed: RFC 4791 PROPFIND',
    snippet: 'Sync collection completed with zero deltas found. All JSCalendar components normalized.',
    bodyHtml: `
      <div style="font-family: sans-serif; color: #1e293b; padding: 16px;">
        <h2>Calendar Sync Completed</h2>
        <p>WebDAV sync-collection (RFC 6578) with sync-token parameters completed successfully.</p>
      </div>
    `,
    date: 'Yesterday',
    read: false,
    folderId: 'inbox',
  },
  {
    id: 'msg-3',
    sender: 'Rust Foundation',
    senderEmail: 'announcements@rust-lang.org',
    subject: 'Nuncio Architecture Compliance Verified',
    snippet: '100% Rust workspace encapsulation with anti-corruption boundary verified.',
    bodyHtml: `
      <div style="font-family: sans-serif; color: #1e293b; padding: 16px;">
        <h2>Hexagonal Boundary Compliance</h2>
        <p>Zero third-party library leakage in nuncio-core domain models verified.</p>
      </div>
    `,
    date: 'Jul 20',
    read: true,
    folderId: 'archive',
  },
];

export default function App() {
  const [folders] = useState<Folder[]>(INITIAL_FOLDERS);
  const [messages, setMessages] = useState<Message[]>(INITIAL_MESSAGES);
  const [activeFolderId, setActiveFolderId] = useState<string>('inbox');
  const [selectedMessageId, setSelectedMessageId] = useState<string | null>('msg-1');
  const [searchQuery, setSearchQuery] = useState<string>('');
  const [status, setStatus] = useState<string>('Ready');

  const handleSync = async () => {
    setStatus('Syncing');
    try {
      await invoke('dispatch_ipc_command', {
        payload: { action: 'sync' },
      });
    } catch {
      // Dev mode fallback when outside Tauri runtime
    } finally {
      setTimeout(() => setStatus('Ready'), 800);
    }
  };

  const handleToggleRead = async (messageId: string, currentReadState: boolean) => {
    const newReadState = !currentReadState;
    setMessages((prev) =>
      prev.map((msg) =>
        msg.id === messageId ? { ...msg, read: newReadState } : msg
      )
    );

    try {
      await invoke('dispatch_ipc_command', {
        payload: {
          action: 'mark_read',
          message_id: messageId,
          read: newReadState,
        },
      });
    } catch {
      // Dev mode fallback
    }
  };

  const folderMessages = useMemo(() => {
    return messages.filter((msg) => {
      const matchesFolder = msg.folderId === activeFolderId;
      if (!matchesFolder) return false;
      if (!searchQuery.trim()) return true;
      const q = searchQuery.toLowerCase();
      return (
        msg.subject.toLowerCase().includes(q) ||
        msg.sender.toLowerCase().includes(q) ||
        msg.snippet.toLowerCase().includes(q)
      );
    });
  }, [messages, activeFolderId, searchQuery]);

  const selectedMessage = useMemo(() => {
    return messages.find((m) => m.id === selectedMessageId) || null;
  }, [messages, selectedMessageId]);

  return (
    <div className="app-container">
      <Sidebar
        folders={folders}
        activeFolderId={activeFolderId}
        onSelectFolder={(fId) => {
          setActiveFolderId(fId);
          const firstInFolder = messages.find((m) => m.folderId === fId);
          setSelectedMessageId(firstInFolder ? firstInFolder.id : null);
        }}
        status={status}
        onSync={handleSync}
      />
      <MessageList
        messages={folderMessages}
        selectedMessageId={selectedMessageId}
        onSelectMessage={(id) => {
          setSelectedMessageId(id);
          const target = messages.find((m) => m.id === id);
          if (target && !target.read) {
            handleToggleRead(id, false);
          }
        }}
        searchQuery={searchQuery}
        onSearchChange={setSearchQuery}
      />
      <Reader message={selectedMessage} onToggleRead={handleToggleRead} />
    </div>
  );
}
