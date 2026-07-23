export interface Message {
  id: string;
  sender: string;
  senderEmail: string;
  subject: string;
  snippet: string;
  bodyHtml: string;
  date: string;
  read: boolean;
  folderId: string;
}

export interface Folder {
  id: string;
  name: string;
  unreadCount: number;
}
