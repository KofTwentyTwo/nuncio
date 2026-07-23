import React from 'react';
import { Message } from '../types';

interface ReaderProps {
  message: Message | null;
  onToggleRead: (messageId: string, currentReadState: boolean) => void;
}

export const Reader: React.FC<ReaderProps> = ({ message, onToggleRead }) => {
  if (!message) {
    return (
      <div className="reader">
        <div className="reader-empty">
          <p>Select a message to view content</p>
        </div>
      </div>
    );
  }

  const initial = message.sender.charAt(0).toUpperCase();

  return (
    <main className="reader">
      <header className="reader-toolbar">
        <div className="toolbar-actions">
          <button
            className="btn"
            onClick={() => onToggleRead(message.id, message.read)}
          >
            {message.read ? 'Mark as Unread' : 'Mark as Read'}
          </button>
        </div>
      </header>

      <section className="reader-header">
        <h2 className="reader-subject">{message.subject}</h2>
        <div className="reader-meta">
          <div className="avatar">{initial}</div>
          <div className="meta-info">
            <div className="meta-sender">
              {message.sender} &lt;{message.senderEmail}&gt;
            </div>
            <div className="meta-recipients">To: me &bull; {message.date}</div>
          </div>
        </div>
      </section>

      <div className="reader-body-container">
        <iframe
          title={message.subject}
          srcDoc={message.bodyHtml}
          sandbox=""
          className="sandboxed-iframe"
        />
      </div>
    </main>
  );
};
