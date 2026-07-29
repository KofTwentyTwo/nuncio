import React from 'react';
import { Message } from '../types';

interface MessageListProps {
  messages: Message[];
  selectedMessageId: string | null;
  onSelectMessage: (messageId: string) => void;
  searchQuery: string;
  onSearchChange: (query: string) => void;
}

export const MessageList: React.FC<MessageListProps> = ({
  messages,
  selectedMessageId,
  onSelectMessage,
  searchQuery,
  onSearchChange,
}) => {
  return (
    <div className="message-list">
      <div className="search-bar-container">
        <div className="search-input-wrapper">
          <input
            type="text"
            className="search-input"
            placeholder="Search mail (FTS5)..."
            value={searchQuery}
            onChange={(e) => onSearchChange(e.target.value)}
          />
        </div>
      </div>

      <div className="messages-scroll">
        {messages.length === 0 ? (
          <div style={{ padding: '24px', textAlign: 'center', color: 'var(--text-muted)' }}>
            No messages found
          </div>
        ) : (
          messages.map((msg) => (
            <div
              key={msg.id}
              className={`message-card ${!msg.read ? 'unread' : ''} ${
                selectedMessageId === msg.id ? 'selected' : ''
              }`}
              onClick={() => onSelectMessage(msg.id)}
            >
              <div className="card-header">
                <span className="message-sender">{msg.sender}</span>
                <span className="message-date">{msg.date}</span>
              </div>
              <div className="message-subject">{msg.subject}</div>
              <div className="message-snippet">{msg.snippet}</div>
            </div>
          ))
        )}
      </div>
    </div>
  );
};
