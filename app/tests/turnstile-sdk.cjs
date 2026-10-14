const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const assert = require('node:assert/strict');

// Browser and external service boundaries only; SDK configuration, submission,
// verification lifecycle and draft handling all run as shipped.
class Element {
  constructor(tag, owner) {
    this.tagName = tag.toUpperCase(); this.ownerDocument = owner;
    this.children = []; this.listeners = new Map(); this.attributes = {};
    this.className = ''; this.textContent = ''; this.value = ''; this.disabled = false;
    this.style = {overflow: '', setProperty(name, value) { this[name] = value; }};
    this.classList = {remove: () => {}};
  }
  appendChild(child) {
    if (this.ownerDocument.body?.contains(child)) this.ownerDocument.connectedMoves = (this.ownerDocument.connectedMoves || 0) + 1;
    child.remove(); this.children.push(child); child.parentNode = this; return child;
  }
  insertBefore(child, reference) { child.remove(); this.children.splice(this.children.indexOf(reference), 0, child); child.parentNode = this; return child; }
  replaceWith(replacement) { if (this.parentNode) { this.parentNode.insertBefore(replacement, this); this.remove(); } }
  get innerHTML() { return this._html || ''; }
  set innerHTML(value) { this._html = value; for (const child of this.children) child.parentNode = null; this.children = []; }
  remove() { if (this.parentNode) { const children = this.parentNode.children; children.splice(children.indexOf(this), 1); this.parentNode = null; } }
  setAttribute(name, value) { this.attributes[name] = String(value); if (name === 'disabled') this.disabled = true; }
  removeAttribute(name) { delete this.attributes[name]; }
  addEventListener(name, fn) { if (!this.listeners.has(name)) this.listeners.set(name, new Set()); this.listeners.get(name).add(fn); }
  removeEventListener(name, fn) { this.listeners.get(name)?.delete(fn); }
  dispatch(name, event = {}) {
    let stopped = false;
    const delivered = {...event, target: event.target || this, preventDefault() {}, stopPropagation() {}, stopImmediatePropagation() { stopped = true; }};
    for (const fn of this.listeners.get(name) || []) { fn(delivered); if (stopped) break; }
  }
  focus() { this.ownerDocument.activeElement = this; }
  contains(el) { return el === this || this.children.some(child => child.contains(el)); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  querySelectorAll(selector) {
    const last = selector.split(' ').at(-1);
    const match = el => last[0] === '.' ? el.className.split(' ').includes(last.slice(1)) : selector.includes('button,') ? ['BUTTON', 'A', 'IFRAME'].includes(el.tagName) : last === 'button' ? el.tagName === 'BUTTON' : false;
    return this.children.flatMap(child => [...(match(child) ? [child] : []), ...child.querySelectorAll(selector)]);
  }
}

function browser() {
  const document = new Element('document', null); document.ownerDocument = document;
  document.createElement = tag => new Element(tag, document);
  document.createTextNode = text => Object.assign(new Element('text', document), {textContent: text});
  document.head = document.appendChild(document.createElement('head'));
  document.body = document.appendChild(document.createElement('body'));
  document.activeElement = document.body;
  let now = 0, timerId = 0;
  const timers = new Map(), posts = [], events = [], widgets = [], executed = [], removed = [];
  let config = {pow: {enabled: false}, image: {enabled: false}, turnstile: {enabled: true, mode: 'managed', site_key: 'public-test-key'}};
  let postResponse = async () => ({status: 200, json: async () => ({pending: true})});
  const api = {
    render(container, options) { const id = 'widget-' + widgets.length; widgets.push({id, container, options}); events.push('render'); return id; },
    execute(id) { executed.push(id); events.push('execute'); },
    remove(id) { removed.push(id); }
  };
  const context = {
    document, console, URL, AbortController,
    getComputedStyle: () => ({length: 0}),
    setTimeout(fn, delay) { const id = ++timerId; timers.set(id, {fn, at: now + delay}); return id; },
    clearTimeout: id => timers.delete(id),
    fetch: async (url, options = {}) => {
      if (options.method === 'POST') { posts.push(JSON.parse(options.body)); events.push('post'); return postResponse(); }
      if (url.endsWith('/pow')) { events.push('pow'); return {ok: true, json: async () => ({challenge: 'test', difficulty: 0})}; }
      return {ok: true, json: async () => config};
    }
  };
  vm.createContext(context);
  vm.runInContext(fs.readFileSync(path.resolve(__dirname, '../../static/sdk/rustaline.js'), 'utf8'), context);
  function instance() {
    const root = document.body.appendChild(document.createElement('div'));
    const form = root.appendChild(document.createElement('form')); form.className = 'rs-form';
    const button = form.appendChild(document.createElement('button')); button.className = 'rs-btn rs-btn--primary';
    const textarea = form.appendChild(document.createElement('textarea')); textarea.value = '草稿内容';
    form._rs_refs = {textarea, nickInput: {value: '作者'}, mailInput: {value: ''}, linkInput: {value: ''}};
    const sdk = Object.create(context.Rustaline.prototype);
    Object.assign(sdk, {lang: context.Rustaline.langs['zh-CN'], opts: {server: '', url: '/test'}, el: root,
      state: {threads: [], rootTotal: 0, count: 0, expandedThreads: {}, replyTo: null},
      draft: {comment: '草稿内容', reply_notification: false}, captcha: {pow: {enabled: false}, image: {enabled: false}, configOk: true, supported: true}});
    sdk._render = () => {};
    sdk._showFormError = (_form, text) => { sdk.error = text; };
    return {sdk, form, button};
  }
  return {context, document, instance, posts, events, widgets, executed, removed, timers, api,
    configure: value => { config = {...config, ...value}; },
    response: fn => { postResponse = fn; },
    load() { context.turnstile = api; const script = document.head.children.find(el => el.tagName === 'SCRIPT'); if (script) { if (script.onload) script.onload(); script.dispatch('load'); } },
    advance(ms) { now += ms; for (const [id, timer] of [...timers]) if (timer.at <= now) { timers.delete(id); timer.fn(); } }
  };
}
const flush = async () => { for (let i = 0; i < 35; i++) await Promise.resolve(); };

async function run() {
  let b = browser(); let {sdk, form} = b.instance();
  sdk._handleSubmit(form); await flush();
  assert.equal(b.posts.length, 0, '启用 Turnstile 后，缺少 token 必须阻止 POST');
  assert.equal(sdk.draft.comment, '草稿内容', '等待安全验证时保留草稿');
  assert.equal(b.document.head.children.length, 1, '按需加载官方脚本');
  b.load(); await flush(); b.widgets[0].options['error-callback'](); await flush();
  assert.equal(b.posts.length, 0, '验证失败不能 POST');
  assert.equal(sdk.draft.comment, '草稿内容', '验证失败保留草稿');
  assert.ok(sdk.error);

  for (const turnstile of [{enabled: false}, undefined]) {
    b = browser(); b.configure({turnstile}); ({sdk, form} = b.instance());
    sdk._handleSubmit(form); await flush();
    assert.equal(b.posts.length, 1, '关闭或旧配置缺少 Turnstile 时兼容提交');
    assert.equal(b.document.head.children.length, 0, '关闭时不加载脚本');
  }

  // Explicit execution and cleanup are essential: rendering alone does not
  // imply a click, and no widget may deliver a token to another SDK instance.
  for (const mode of ['managed', 'invisible', 'click']) {
    b = browser(); b.configure({turnstile: {enabled: true, mode, site_key: 'public-test-key'}});
    ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush(); b.load(); await flush();
    const widget = b.widgets[0];
    assert.equal(widget.options.action, 'comment');
    assert.equal(widget.options.execution, 'execute');
    assert.equal(widget.options.appearance, mode === 'managed' ? 'interaction-only' : 'always');
    assert.equal(widget.options.retry, 'never');
    assert.equal(widget.options['refresh-expired'], 'manual');
    assert.equal(widget.options['response-field'], false);
    assert.ok(b.document.body.contains(widget.container), '组件挂 body，表单重绘不丢失');
    if (mode === 'click') {
      assert.equal(b.executed.length, 0, '必须点击模式不能提前 execute');
      const verify = b.document.querySelector('.rs-turnstile-start');
      assert.ok(verify); verify.dispatch('click'); verify.dispatch('click');
      assert.equal(b.executed.length, 1, '重复点击验证只执行一次');
    } else {
      assert.equal(b.executed.length, 1);
      assert.equal(b.document.querySelector('.rs-turnstile-modal'), null, '非点击模式不预先打开模态框');
      if (mode === 'managed') {
        widget.options['before-interactive-callback']();
        assert.ok(b.document.querySelector('.rs-turnstile-modal'), '官方组件要求交互才显示对话框');
      }
    }
    sdk._handleSubmit(form); await flush();
    assert.equal(b.widgets.length, 1, '重复提交只保留一个验证生命周期');
    widget.options.callback(mode + '-token'); await flush();
    assert.equal(b.posts.length, 1); assert.equal(b.posts[0].turnstile_token, mode + '-token');
    assert.equal(b.removed.length, 1, '成功清理组件');
    assert.equal(b.document.querySelector('.rs-turnstile-modal'), null);
    widget.options.callback('stale'); await flush(); assert.equal(b.posts.length, 1, '迟到回调不重复提交');
  }

  b = browser(); ({sdk, form} = b.instance());
  const second = b.instance(); sdk._handleSubmit(form); second.sdk._handleSubmit(second.form); await flush();
  assert.equal(b.document.head.children.length, 1, '多实例共享同一脚本');
  b.load(); await flush();
  b.widgets[1].options.callback('second-token'); await flush();
  assert.equal(b.posts[0].turnstile_token, 'second-token'); assert.equal(sdk.draft.comment, '草稿内容');
  b.widgets[0].options.callback('first-token'); await flush();
  assert.equal(b.posts[1].turnstile_token, 'first-token');

  for (const failure of ['error-callback', 'expired-callback', 'timeout-callback', 'cancel', 'destroy', 'wait-timeout']) {
    b = browser(); ({sdk, form} = b.instance());
    const reply = {id: 'parent', rid: 'root'}; sdk.state.replyTo = reply;
    sdk.captcha.captchaId = 'old-image'; sdk.captcha.captchaImage = 'old-image-data';
    sdk._handleSubmit(form); await flush(); b.load(); await flush();
    const widget = b.widgets[0];
    if (failure === 'cancel') b.document.querySelector('.rs-turnstile-cancel').dispatch('click');
    else if (failure === 'destroy') sdk.destroy();
    else if (failure === 'wait-timeout') b.advance(120000);
    else widget.options[failure]();
    await flush(); widget.options.callback('late-token'); await flush();
    assert.equal(b.posts.length, 0, failure + ' 不 POST');
    assert.equal(sdk.draft.comment, '草稿内容'); assert.equal(sdk.state.replyTo, reply);
    assert.equal(b.removed.length, 1, failure + ' 清理 widget');
    assert.equal(sdk.captcha.captchaId, '', failure + ' 不复用图形码');
    assert.equal(b.document.querySelector('.rs-turnstile-cancel'), null, failure + ' 清理取消按钮');
    assert.equal(b.timers.size, 0, failure + ' 清理验证与脚本定时器');
  }

  for (const failure of ['script-error', 'script-timeout', 'cancel-loading', 'destroy-loading']) {
    b = browser(); ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush();
    const firstScript = b.document.head.children[0];
    if (failure === 'script-error') firstScript.onerror();
    else if (failure === 'script-timeout') b.advance(10000);
    else if (failure === 'cancel-loading') b.document.querySelector('.rs-turnstile-cancel').dispatch('click');
    else sdk.destroy();
    await flush(); assert.equal(b.posts.length, 0); assert.equal(sdk.draft.comment, '草稿内容');
    if (failure.startsWith('script')) {
      assert.equal(b.document.head.children.length, 0, '失败脚本移除，可重新加载');
      sdk._handleSubmit(form); await flush();
      assert.equal(b.document.head.children.length, 1, '脚本加载失败允许重试');
      b.load(); await flush(); b.widgets[0].options.callback('retry'); await flush(); assert.equal(b.posts.length, 1);
    } else { b.load(); await flush(); assert.equal(b.widgets.length, 0, '取消或销毁加载时不得继续 render'); }
  }

  b = browser(); ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush(); b.load(); await flush();
  sdk.draft.comment = '新草稿'; const newReply = {id: 'new-parent'}; sdk.state.replyTo = newReply;
  b.widgets[0].options.callback('valid'); await flush();
  assert.equal(sdk.draft.comment, '新草稿', '成功不能清空异步等待时的新草稿');
  assert.equal(sdk.state.replyTo, newReply, '成功不能清空异步等待时的新回复目标');
  assert.equal(b.posts[0].comment, '草稿内容', '提交仍使用原快照');

  b = browser(); ({sdk, form} = b.instance());
  b.response(async () => ({status: 400, json: async () => ({message: 'Turnstile validation failed'})}));
  sdk._handleSubmit(form); await flush(); b.load(); await flush(); b.widgets[0].options.callback('consumed'); await flush();
  assert.equal(b.posts.length, 1, '服务端拒绝绝不自动重发');
  assert.equal(sdk.draft.comment, '草稿内容');
  sdk._handleSubmit(form); await flush(); assert.equal(b.widgets.length, 2, '失败重试创建新 widget');
  b.widgets[1].options.callback('fresh'); await flush(); assert.equal(b.posts[1].turnstile_token, 'fresh');
  b.configure({turnstile: {enabled: false}}); sdk._handleSubmit(form); await flush();
  assert.equal(b.widgets.length, 2); assert.equal(b.posts.length, 3, '动态关闭后停止获取 token');

  b = browser(); b.configure({turnstile: {enabled: true, mode: 'click', site_key: 'public-test-key'}});
  ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush();
  assert.equal(b.document.querySelector('.rs-turnstile-start').disabled, true, '脚本加载前禁用点击按钮');
  b.load(); await flush();
  const oldWidget = b.widgets[0];
  b.configure({turnstile: {enabled: true, mode: 'invisible', site_key: 'new-key'}});
  await sdk._probeCaptchaConfig(5000); await flush();
  oldWidget.options.callback('old-config-token'); await flush();
  assert.equal(b.posts.length, 0, '验证期间切换配置必须取消旧验证');
  sdk._handleSubmit(form); await flush(); assert.equal(b.widgets[1].options.sitekey, 'new-key');
  b.widgets[1].options.callback('new-config-token'); await flush(); assert.equal(b.posts.length, 1);

  b = browser(); ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush(); b.load(); await flush();
  const rebuiltForm = sdk._buildForm();
  assert.ok(rebuiltForm.querySelector('.rs-turnstile-cancel'), '表单重绘仍可取消挂起验证');
  rebuiltForm.querySelector('.rs-turnstile-cancel').dispatch('click'); await flush(); assert.equal(b.posts.length, 0);

  b = browser(); ({sdk, form} = b.instance());
  b.configure({image: {enabled: true}, pow: {enabled: true, difficulty: 0}});
  sdk._openCaptchaModal = async () => { b.events.push('image'); sdk.captcha.captchaId = 'image-id'; return 'abcd'; };
  sdk._handleSubmit(form); await flush(); b.load(); await flush(); b.widgets[0].options.callback('all'); await flush();
  assert.deepEqual(b.events, ['image', 'pow', 'render', 'execute', 'post']);
  assert.equal(b.posts[0].captcha_id, 'image-id'); assert.equal(b.posts[0].captcha_code, 'abcd');

  b = browser(); b.configure({turnstile: {enabled: true, mode: 'click', site_key: 'public-test-key'}});
  ({sdk, form} = b.instance()); const other = b.instance();
  sdk._handleSubmit(form); other.sdk._handleSubmit(other.form); await flush(); b.load(); await flush();
  assert.equal(b.document.body.style.overflow, 'hidden');
  b.widgets[0].options['error-callback'](); await flush();
  assert.equal(b.document.body.style.overflow, 'hidden', '关闭一个实例弹窗不能解除另一个实例滚动锁');
  b.document.dispatch('keydown', {key: 'Escape'}); await flush();
  assert.equal(b.document.body.style.overflow, ''); assert.equal(b.posts.length, 0);

  b = browser(); ({sdk, form} = b.instance());
  let releasePow; sdk._solvePow = () => new Promise(resolve => { releasePow = resolve; });
  b.configure({pow: {enabled: true, difficulty: 0}}); sdk._handleSubmit(form); await flush();
  sdk.destroy(); releasePow({challenge: 'test', nonce: 0}); await flush();
  assert.equal(b.posts.length, 0); assert.equal(b.document.head.children.length, 0, '销毁后 PoW 完成不加载脚本');

  for (const token of ['', '   ', 'x'.repeat(2049), null]) {
    b = browser(); ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush(); b.load(); await flush();
    b.widgets[0].options.callback(token); await flush();
    assert.equal(b.posts.length, 0, '非法 token 不提交'); assert.equal(b.removed.length, 1);
  }
  b = browser(); b.configure({turnstile: {enabled: true, mode: 'invisible', site_key: 'wrong-managed-key'}});
  ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush(); b.load(); await flush();
  b.widgets[0].options['before-interactive-callback'](); await flush();
  assert.equal(b.posts.length, 0); assert.equal(b.removed.length, 1, '隐藏模式遇到交互组件必须失败，不用 CSS 隐藏它');
  assert.ok(sdk.error);

  for (const boundary of ['render', 'execute']) {
    b = browser(); b.api[boundary] = () => { throw Error('external failure'); };
    ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush(); b.load(); await flush();
    assert.equal(b.posts.length, 0); assert.equal(sdk.draft.comment, '草稿内容'); assert.ok(sdk.error);
    assert.equal(b.document.querySelector('.rs-turnstile-widget'), null, boundary + ' 抛异常也清理 DOM');
    assert.equal(b.timers.size, 0);
  }
  b = browser(); b.configure({turnstile: {enabled: true, mode: 'click', site_key: 'public-test-key'}});
  ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush(); b.load(); await flush();
  b.widgets[0].options.callback('unsolicited'); await flush();
  assert.equal(b.posts.length, 0, '必须点击模式不能接受主动点击之前的 token');

  for (const replacement of ['草稿内容', '']) {
    b = browser(); ({sdk, form} = b.instance());
    sdk._handleSubmit(form); await flush(); b.load(); await flush();
    const nextReply = {id: 'next-parent'}; sdk.state.replyTo = nextReply; sdk.draft.comment = replacement;
    b.widgets[0].options.callback('snapshot-token'); await flush();
    assert.equal(sdk.state.replyTo, nextReply); assert.equal(sdk.draft.comment, replacement, '切换回复后同文草稿或手动清空也必须保留');
  }

  b = browser(); b.configure({pow: {enabled: true, difficulty: 0}}); ({sdk, form} = b.instance());
  sdk._handleSubmit(form); await flush(); assert.equal(b.events[0], 'pow', 'PoW 完成之后才加载 Turnstile');
  b.load(); await flush(); b.widgets[0].options.callback('ordered'); await flush();
  assert.deepEqual(b.events, ['pow', 'render', 'execute', 'post']); assert.ok(b.posts[0].pow);
  b = browser(); ({sdk, form} = b.instance()); sdk._handleSubmit(form); await flush(); b.load(); await flush();
  const interactive = b.widgets[0];
  const renderedParent = interactive.container.parentNode;
  const iframe = interactive.container.appendChild(b.document.createElement('iframe'));
  const connectedMoves = b.document.connectedMoves || 0;
  interactive.options['before-interactive-callback']();
  assert.equal(b.document.connectedMoves || 0, connectedMoves, '显示交互框不得断开并重新挂载任何已连接节点');
  assert.ok(interactive.container.parentNode === renderedParent, '显示交互框不能重新挂载已 render 的 iframe 宿主');
  assert.equal(iframe.parentNode, interactive.container);
  assert.ok(b.document.body.contains(iframe));
  interactive.options['error-callback'](); await flush();
  b = browser(); ({sdk, form} = b.instance());
  const sameParent = {id: 'same-parent'}; sdk.state.replyTo = sameParent;
  sdk._handleSubmit(form); await flush(); b.load(); await flush();
  sdk.draft.comment = '发给同一人的下一条回复';
  b.widgets[0].options.callback('first-reply'); await flush();
  assert.equal(sdk.draft.comment, '发给同一人的下一条回复');
  assert.ok(sdk.state.replyTo === sameParent, '新正文与同一回复目标必须成对保留');
  b = browser(); ({sdk, form} = b.instance()); const later = b.instance();
  sdk._handleSubmit(form); later.sdk._handleSubmit(later.form); await flush(); b.load(); await flush();
  b.widgets[1].options['before-interactive-callback']();
  b.widgets[0].options['before-interactive-callback']();
  b.document.dispatch('keydown', {key: 'Escape'}); await flush();
  assert.ok(sdk._turnstileAttempt === null, '倒序出现的弹框以最后显示的实例为顶层');
  assert.ok(later.sdk._turnstileAttempt, '一次 Escape 不能继续取消刚成为顶层的第二个实例');
  assert.equal(b.removed.length, 1); assert.equal(b.document.body.style.overflow, 'hidden');
  assert.ok(b.document.querySelector('.rs-turnstile-modal').contains(b.document.activeElement), '关闭顶层后焦点留在剩下的弹框');
  b.document.dispatch('keydown', {key: 'Escape'}); await flush();
  assert.equal(b.removed.length, 2); assert.equal(b.document.body.style.overflow, '');

  b = browser(); ({sdk, form} = b.instance()); const focused = b.instance();
  sdk._handleSubmit(form); focused.sdk._handleSubmit(focused.form); await flush(); b.load(); await flush();
  b.widgets[0].options['before-interactive-callback'](); b.widgets[1].options['before-interactive-callback']();
  const topFocus = b.document.activeElement;
  b.widgets[0].options['error-callback'](); await flush();
  assert.ok(b.document.activeElement === topFocus, '下层弹窗异步失败不能抢顶层焦点');
  b.widgets[1].options['error-callback'](); await flush();
  for (const lateResult of ['success', 'body-error', 'fetch-error']) {
    b = browser(); ({sdk, form} = b.instance()); b.configure({image: {enabled: true}});
    const originalFetch = b.context.fetch;
    let releaseImage, rejectImage;
    const delayedImage = new Promise((resolve, reject) => { releaseImage = resolve; rejectImage = reject; });
    b.context.fetch = (url, options) => url.endsWith('/image')
      ? (lateResult === 'fetch-error' ? delayedImage : Promise.resolve({ok: true, json: () => delayedImage}))
      : originalFetch(url, options);
    // Use the real modal, image loading and renderer here: a late render would
    // visibly rebuild the destroyed mounting point, rather than hit a spy.
    sdk._render = b.context.Rustaline.prototype._render;
    sdk._handleSubmit(form); await flush(); assert.ok(sdk._captchaModal);
    sdk.destroy(); assert.equal(sdk.el.children.length, 0);
    if (lateResult === 'success') releaseImage({captcha_id: 'late-image', image: 'late-png'});
    else rejectImage(Error('late image failure'));
    await flush();
    assert.equal(sdk.el.children.length, 0, lateResult + ' 图片响应不能复活销毁的根节点');
    assert.equal(b.posts.length, 0); assert.equal(b.document.querySelector('.rs-captcha-overlay'), null);
    assert.equal(sdk.captcha.captchaId, '', '销毁后不接收新的图片凭据');
    sdk._render(); assert.equal(sdk.el.children.length, 0, '销毁后的显式重绘也保持空根节点');
  }
  console.log('Turnstile SDK 三模式、提交门控、脚本重试、多实例、生命周期、草稿与回复保护回归通过');
}

if (require.main === module) run().catch(err => {console.error(err); process.exitCode = 1;});
module.exports = run;
