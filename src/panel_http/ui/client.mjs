import { id, operationId, integer, requireThat, validateReply, validateError, ProtocolError } from './core.mjs';
import { ENUMS, LIMITS } from './contract.mjs';

const routeBrand = Symbol('fixed-route');
const prefix = '/api/admin/v1';
const MAX_REQUEST = 1024 * 1024, MAX_RESPONSE = 32 * 1024 * 1024;
export class ClientError extends Error {
  constructor(code, status = null, existingSourceId = null) {
    super(code === 'deadline' ? '请求等待超时；写入结果需要核对。' : code === 'cancelled' ? '请求已失效。' : code === 'transport' ? '连接失败；写入结果需要核对。' : `请求失败：${code}`);
    this.name = 'ClientError'; this.code = code; this.status = status; this.existingSourceId = existingSourceId;
  }
}
// 调用者不能提供任意路径、host、方法或响应族。
export function route(action, args = {}) {
  let path, method = 'GET', family, write = false, status = 200;
  const entity = () => id(args.id), op = () => operationId(args.id);
  switch (action) {
    case 'config': path = '/ui-config'; family = 'config'; break;
    case 'state': path = '/state'; family = 'state'; break;
    case 'createSource': path = '/sources'; method = 'POST'; family = 'sourceCreated'; write = true; status = 201; break;
    case 'sourceModels': path = `/sources/${entity()}/models`; family = 'sourceModels'; break;
    case 'setModels': path = `/sources/${entity()}/models`; method = 'PUT'; family = 'operation'; write = true; break;
    case 'discoverModels': path = `/sources/${entity()}/models/discover`; method = 'POST'; family = 'operation'; write = true; break;
    case 'setSourcePaused': path = `/sources/${entity()}/paused`; method = 'PUT'; family = 'operation'; write = true; break;
    case 'retireSource': path = `/sources/${entity()}/retire`; method = 'POST'; family = 'operation'; write = true; break;
    case 'clearError': path = `/sources/${entity()}/clear-error`; method = 'POST'; family = 'operation'; write = true; break;
    case 'setVolcanoPolicy': path = `/sources/${entity()}/volcano-policy`; method = 'PUT'; family = 'operation'; write = true; break;
    case 'createGroup': path = '/groups'; method = 'POST'; family = 'groupCreated'; write = true; status = 201; break;
    case 'checkGroup': path = `/groups/${entity()}/check`; method = 'POST'; family = 'groupCheck'; write = true; break;
    case 'groupReferences': path = `/groups/${entity()}/references`; family = 'groupReferences'; break;
    case 'setGroupPin': path = `/groups/${entity()}/pin`; method = 'PUT'; family = 'operation'; write = true; break;
    case 'planMember': path = '/memberships'; method = 'POST'; family = 'memberPlanned'; write = true; status = 201; break;
    case 'ensureMember': path = `/memberships/${entity()}/ensure`; method = 'POST'; family = 'operation'; write = true; break;
    case 'retireMember': path = `/memberships/${entity()}/retire`; method = 'POST'; family = 'operation'; write = true; break;
    case 'operation': path = `/operations/${op()}`; family = 'operation'; break;
    case 'retry': path = `/operations/${op()}/retry`; method = 'POST'; family = 'operation'; write = true; break;
    case 'identities': path = `/tokens${args.after == null ? '' : `?after=${id(args.after)}`}`; family = 'identities'; break;
    case 'identity': path = `/tokens/${entity()}`; family = 'identity'; break;
    case 'createToken': path = '/tokens'; method = 'POST'; family = 'operation'; write = true; break;
    case 'setTokenEnabled': path = `/tokens/${entity()}/enabled`; method = 'PUT'; family = 'operation'; write = true; break;
    case 'deleteToken': path = `/tokens/${entity()}`; method = 'DELETE'; family = 'operation'; write = true; break;
    case 'registerToken': path = '/tokens/register'; method = 'POST'; family = 'identity'; write = true; break;
    case 'reveal': path = `/tokens/${entity()}/reveal`; method = 'POST'; family = 'reveal'; break;
    case 'nativePage':
      requireThat(ENUMS.ownerScope.includes(args.owner_scope));
      integer(args.page, 1, LIMITS.nativePages);
      path = `/native-tokens?owner_scope=${args.owner_scope}&page=${args.page}`; family = 'nativePage'; break;
    case 'usage': path = '/usage/query'; method = 'POST'; family = 'usageSnapshot'; break;
    default: throw new ProtocolError();
  }
  if (family === 'operation' && method !== 'GET' && action !== 'retry') status = 202;
  return Object.freeze({ [routeBrand]: true, path: prefix + path, action, method, family, write, status, query: Object.freeze({ ...args }) });
}
export function serializeBody(body) {
  let encoded;
  try { encoded = JSON.stringify(body); } catch { throw new ProtocolError(); }
  requireThat(typeof encoded === 'string' && new TextEncoder().encode(encoded).length <= MAX_REQUEST);
  return encoded;
}
async function readBounded(response, check) {
  requireThat(response.body !== null && typeof response.body.getReader === 'function');
  const reader = response.body.getReader();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  let size = 0, text = '', finished = false;
  try {
    while (true) {
      check();
      const { done, value } = await reader.read();
      check();
      if (done) { finished = true; break; }
      size += value.byteLength;
      requireThat(size <= MAX_RESPONSE);
      text += decoder.decode(value, { stream: true });
    }
    text += decoder.decode();
    check();
    let value;
    try { value = JSON.parse(text); } catch { throw new ProtocolError(); }
    text = ''; check(); return value;
  } catch (error) {
    if (error instanceof TypeError) throw new ProtocolError();
    throw error;
  } finally {
    text = '';
    if (!finished) { try { await reader.cancel(); } catch { /* No raw network message is shown. */ } }
    reader.releaseLock();
  }
}
export async function requestJson(descriptor, body, ticket, {
  fetchImpl = globalThis.fetch, now = () => performance.now(), valid,
  setTimer = globalThis.setTimeout, clearTimer = globalThis.clearTimeout,
} = {}) {
  // Step U2: 一条被 await 的 fetch/解码链；abort 不提前归还 owner 的槽。
  requireThat(descriptor?.[routeBrand] === true && typeof valid === 'function');
  const check = () => {
    const time = now();
    if (!Number.isFinite(time) || time < ticket.started || time >= ticket.deadline) throw new ClientError('deadline');
    if (!valid(ticket) || ticket.controller.signal.aborted) throw new ClientError('cancelled');
  };
  check();
  let serialized = descriptor.method === 'GET' ? undefined : serializeBody(body);
  requireThat(descriptor.method !== 'GET' || body == null);
  const headers = { 'X-QT-Panel': '1' };
  if (serialized !== undefined) headers['Content-Type'] = 'application/json';
  const timer = setTimer(() => ticket.controller.abort(), Math.max(0, ticket.deadline - now()));
  let response;
  try {
    check();
    response = await fetchImpl(descriptor.path, {
      method: descriptor.method, body: serialized, headers, signal: ticket.controller.signal,
      cache: 'no-store', credentials: 'omit', redirect: 'error',
    });
    serialized = undefined; check();
    requireThat(!response.redirected && /^application\/json(?:\s*;|$)/i.test(response.headers.get('Content-Type') ?? ''));
    const value = await readBounded(response, check);
    check();
    if (!response.ok) {
      const error = validateError(value);
      throw new ClientError(error.error, response.status, error.existing_source_id ?? null);
    }
    requireThat(response.status === descriptor.status);
    const projected = validateReply(descriptor.family, value, descriptor.action === 'usage' ? body : descriptor.query);
    check();
    return { value: projected, age: response.headers.get('X-QT-Snapshot-Age-Ms'), received: now() };
  } catch (error) {
    const wasCancelled = ticket.controller.signal.aborted || !valid(ticket);
    // 头部或截止检查可在 getReader 前失败，此时也要关闭未消费的响应体。
    ticket.controller.abort();
    if (response?.body && !response.body.locked && !response.bodyUsed) {
      try { await response.body.cancel(); } catch { /* Fixed error only. */ }
    }
    if (error instanceof ProtocolError || error instanceof ClientError) throw error;
    if (now() >= ticket.deadline) throw new ClientError('deadline');
    if (wasCancelled) throw new ClientError('cancelled');
    throw new ClientError('transport');
  } finally {
    serialized = undefined;
    clearTimer(timer);
  }
}
