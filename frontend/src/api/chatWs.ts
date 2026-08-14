/** URL du WS chat pour une conversation. Same-origin (contrairement à
 *  `sandboxWs.ts` : pas d'ingress séparé, pas de ticket — l'auth passe par
 *  le cookie OIDC, envoyé automatiquement par le navigateur sur un
 *  handshake WS same-origin). */
export function chatWsUrl(conversationId: string): string {
  const protocol = globalThis.location.protocol === 'https:' ? 'wss' : 'ws';
  return `${protocol}://${globalThis.location.host}/api/ws/chat/${conversationId}`;
}

/** N'ouvre la connexion qu'au véritable event `open` (pas à la
 *  construction, encore `CONNECTING`) — même raison qu'`openSandboxWs`
 *  (cf. `sandboxWs.ts`) : envoyer sur un socket `CONNECTING` lève une
 *  `InvalidStateError`. */
export async function openChatWs(conversationId: string): Promise<WebSocket> {
  const ws = new WebSocket(chatWsUrl(conversationId));
  return new Promise<WebSocket>((resolve, reject) => {
    const onOpen = () => {
      ws.removeEventListener('error', onError);
      resolve(ws);
    };
    const onError = (ev: Event) => {
      ws.removeEventListener('open', onOpen);
      reject(new Error(`chat WebSocket: ${ev.type}`));
    };
    ws.addEventListener('open', onOpen, { once: true });
    ws.addEventListener('error', onError, { once: true });
  });
}
