(function () {
  'use strict';

  // English stays in the HTML, so the documentation also works without JavaScript.
  // Only these repository-owned translations are inserted as HTML.
  var zh = {
    inzoneTitle: 'Sony INZONE 加入设备中心',
    inzoneAlt: 'Mirage Hub：Sony INZONE H5 音频与电源设置',
    upgrade: '从 RsRazer 升级？首次请手动运行 <code>mirage-hub-setup.exe</code>。设置会复制到 <code>%LOCALAPPDATA%\\mirage-hub</code>，原文件仍会保留。',
    inzone: 'INZONE 控制移植自 <a href="https://github.com/penguinwokrs/openinzone">OpenInzone</a>。H5 的音量、游戏/聊天平衡、侧音与自动关机计时已通过实机验证。Buds、H9 与 H9 II 使用上游协议，按耳机实际返回的能力显示控制项。H9/H7 需关闭 INZONE Hub，因为串口只能独占使用。不包含 Sony APO 均衡器和空间音效。',
    description: 'Mirage Hub 在 Windows 10 和 11 上统一控制 Razer 外设与 Sony INZONE 耳机，采用原生 Rust 程序，无需自定义内核驱动。',
    ogDescription: '用一个 Windows 程序控制 Razer 与 Sony INZONE。原生 Rust，无需自定义内核驱动。',
    install: '安装', usage: '使用', how: '工作原理', devices: '设备', help: '故障排查',
    language: '页面语言', auto: '自动',
    hero: '你的 Windows 设备，<br><span>一个 Mirage Hub。</span>',
    lead: '用原生 Rust 程序统一控制 Razer 外设与 Sony INZONE 耳机。基于 <a href="https://github.com/openrazer/openrazer">OpenRazer</a> 和 <a href="https://github.com/penguinwokrs/openinzone">OpenInzone</a>，按型号显示实际支持的设置，无需自定义内核驱动。',
    deviceTotal: '289 个产品 ID', native: '原生 Rust 后端', noDriver: '无需内核驱动',
    noAdmin: '无需管理员权限', noTelemetry: '无遥测', noPython: '无需 Python 运行时',
    getStarted: '开始使用', source: '查看源码', supportedQuestion: '支持我的设备吗？',
    installIntro: '适用于 Windows 10 / 11（amd64）。Mirage Hub、后台服务和安装程序均为原生 Rust 程序。',
    commentInstall: '# 安装后启动后台服务，并列出已连接的设备',
    download: '从 <a href="https://github.com/imacte/MirageHub/releases">发布页面</a>下载 <code>mirage-hub-setup.exe</code>。安装包包含 Mirage Hub 和 Rust 后台服务，支持英语、俄语和简体中文。内置低功耗蓝牙支持，无需安装 Python、额外软件包或驱动。<code>mirage-hub-daemon.exe</code> 可独立用作命令行工具和后台服务。应用与后端均使用 Rust 原生运行。',
    closeSynapse: '<strong>请先关闭 Razer Synapse。</strong>Synapse 会占用设备，并与 Mirage Hub 争夺灯光控制权。运行 <code>mirage-hub-daemon doctor</code> 可以检查。',
    usageIntro: '提供命令行、JSON-RPC API 和图形界面，按自己的习惯选择。',
    cli: '命令行', gui: '图形界面',
    commentStart: '# 启动后台服务，然后查看设备', commentLighting: '# 灯光',
    commentRipple: '# 随按键产生涟漪', commentZones: '# 每个区域一种颜色；蓝牙耳机可分别设置左右耳颜色',
    commentProfiles: '# 电脑上的当前配置；后续修改会自动保存', commentSettings: '# 设置',
    commentAutostart: '# 随 Windows 启动后台服务（仅当前用户，无需管理员权限）',
    cliHelp: '命令支持 <code>--device</code>（序列号、产品 ID 或部分设备名称）、<code>--zone</code> 和 <code>--json</code> 参数。开机自启只写入一条 <code>HKCU\\…\\Run</code> 注册表记录，因此也会显示在任务管理器的“启动应用”中。',
    commentRpc: '# 查询桌面应用使用的同一个 JSON-RPC 服务',
    rpcHelp: '后台服务通过本机回环 TCP 提供以换行分隔的 JSON-RPC 接口。本地端点文件包含端口和每个进程独立的认证令牌，任何语言都可以实现客户端。硬件控制、灯光、宏和蓝牙功能均由 Rust 实现。',
    guiHelp: '发布包中的 Mirage Hub 会显示设备的产品图片和当前灯光，支持一键切换灯效与颜色、分层逐键灯光、DPI 与档位、电量、随前台应用切换的配置，以及宏库。界面提供英语、简体中文和俄语。安装程序无需管理员权限，自带后台服务，可在<em>设置 › 应用</em>中修复或卸载。发布包仍提供单文件经典面板 <code>mirage-hub-classic.exe</code>。',
    lightingAlt: 'Mirage Hub：分层逐键灯光编辑', homeAlt: 'Mirage Hub：所有设备一览',
    howIntro: 'Linux 版本由内核模块和 D-Bus 后台服务组成。Windows 没有这套组件，因此为每一层提供了对应的替代实现。',
    architectureAlt: 'Linux 与 Windows 分层架构对照，两侧使用相同的 Razer 厂商协议',
    linux: 'LINUX（上游）', windows: 'WINDOWS（本移植）', dbus: 'D-Bus 会话总线', sysfs: 'sysfs 属性、usb_control_msg()',
    kernelSpace: '内核态 — 需要 DKMS', nativeCli: 'Mirage Hub / 原生命令行', loopback: '本机回环 TCP 上的 JSON-RPC',
    tokenWatcher: '认证令牌、热插拔监控', interpreter: 'Rust 设备 API / 协议配方解释器',
    bundledRecipes: '内置从上游 C 源码生成的协议配方', userSpace: '用户态 — 原生 Rust，无需重启', transpiled: '转译',
    vendorProtocol: 'Razer 厂商协议 — 90 字节 HID 功能报告，对第 2–87 字节进行 XOR 校验',
    architectureCaption: '两侧使用相同的协议，仅传输方式和设备特有逻辑的存储形式不同。',
    quirksTitle: '关键之处：转译设备特有逻辑',
    quirksIntro: 'OpenRazer 驱动通过 <code>switch (device-&gt;usb_pid)</code> 语句描述约 20,000 行设备特有行为，包括使用哪种报告构造函数、LED ID 和事务 ID。逐一手工移植很容易与上游产生偏差。',
    recipesIntro: '内置数据从上游 C 源码生成，将已知产品 ID 对应的每个 switch 分支解析为一段小程序，按“属性、设备”组合存储：',
    recipesHelp: '原生 Rust 解释器执行这些内置协议配方。原始生成器保留在 Git 历史中；新增设备支持需要更新数据和回归测试样本。',
    findingTitle: '定位设备',
    finding: '内核可以直接访问 USB 接口；Windows 则将接口暴露为独立的 HID 集合。因此，本移植会选择路径包含匹配的 <code>&amp;mi_XX</code>、<em>同时</em>声明了 90 字节功能报告的集合。',
    exclusiveTitle: '设备独占访问',
    exclusive: 'Windows 不允许直接读写键盘和鼠标集合。但功能报告的 IOCTL 使用 <code>FILE_ANY_ACCESS</code>，因此可以回退到无读写访问权限的句柄，继续与设备通信。',
    testingTitle: '无需硬件也能测试',
    testing: '内置模拟器校验 CRC 并响应协议。315 项测试加上设备群模拟，在每次提交时覆盖全部 276 款 USB 设备的各项能力。',
    rippleTitle: 'Windows 上的涟漪灯效',
    ripple: '上游通过读取 <code>/dev/input</code> 确定涟漪位置。这里使用低级键盘钩子观察按键，再由电脑渲染灯效帧；涟漪功能本身不会拦截或模拟按键输入。',
    sleepTitle: '睡眠后恢复设置',
    sleep: 'Windows 挂起时会切断 USB 电源。后台服务按设备序列号记录已应用的设置，并在设备重新出现时恢复。',
    krakenTitle: '另一套协议',
    kraken: '早期 Kraken 耳机没有使用上述控制报告，而是暴露灯光控制器的 RAM。颜色写入固定地址，再用一个字节指定操作。这部分从 <code>razerkraken_driver.c</code> 手工移植，因为没有可转译的 switch 表。',
    bluetoothTitle: '未公开的蓝牙协议',
    bluetooth: 'Kraken Kitty V2 BT 不支持 USB 数据传输，上游也没有相应驱动。项目通过蓝牙 HCI 抓包还原了灯光协议：厂商 GATT 服务的 UUID 拼出 <code>Amel-RazerBLE</code>，接收 <code>c4 00 06</code> 和每只耳朵的一组 RGB 值。一次写入即可分别设置左右耳颜色。耳机不保存这些设置，后台服务运行期间会维持颜色，也可以用命名配置保存不同方案。',
    blacksharkTitle: '没有灯光的耳机',
    blackshark: 'BlackShark V2 HyperSpeed 基于 MediaTek 方案，使用带序列号和校验和的 64 字节报告。接收器会自行回应部分请求，并转发其余请求。它没有灯光，但耳机内保存了侧音、均衡器和睡眠计时器设置，均可通过 <code>mirage-hub-daemon headset</code> 和 Mirage Hub 调整。',
    chromaTitle: '无需 Synapse 的游戏 Chroma 灯效',
    chroma: '后台服务在 Razer Chroma SDK 的常用端口提供 REST API。将兼容 SDK 的 DLL 放在游戏旁边，即可转发游戏灯效，控制键盘每个按键、鼠标标志与滚轮、耳机左右耳等区域；游戏退出后恢复你自己的灯光。',
    localTitle: '仅限本机',
    local: '后台服务仅绑定本机回环地址，将随机认证令牌写入仅当前用户可访问的文件，只开放固定白名单中的方法，不连接外部网络。',
    supported: '支持的设备', devicesIntro: '设备列表来自上游设备类，覆盖其已知的产品 ID。可以按名称搜索，或按类别筛选。',
    searchPlaceholder: '搜索 289 个产品 ID，例如“BlackWidow”“Viper”“0203”…', searchLabel: '搜索设备',
    all: '全部', keyboards: '键盘', mice: '鼠标', mousepads: '鼠标垫', keypads: '单手键盘',
    headsets: '耳机', accessories: '配件', device: '设备', type: '类型', productId: '产品 ID',
    matrix: '灯光矩阵', maxDpi: '最高 DPI', battery: '电池',
    doctorIntro: '先运行这个命令：它会列出 Windows 能看到的所有 Razer HID 集合，并标记控制接口。',
    symptom: '现象', cause: '原因与处理方法',
    interfaceHelp: 'Synapse 正在运行，或其他程序占用了设备。请先关闭占用设备的程序。',
    unknownDevice: '设备显示为 <code>NOT IN DATABASE</code>',
    unknownHelp: '设备比内置数据更新。请携带产品 ID <a href="https://github.com/imacte/MirageHub/issues">提交问题</a>。',
    unsupportedHelp: '固件拒绝了该命令，设备不支持对应功能。',
    sleepSymptom: '睡眠后灯效重置', sleepHelp: 'Windows 会切断 USB 电源。后台服务会在重新连接时恢复已保存的状态。',
    bluetoothSymptom: '蓝牙设备已开机，但没有出现在列表中',
    bluetoothHelp: 'Rust 后台服务内置低功耗蓝牙支持。运行 <code>mirage-hub-daemon doctor</code> 检查已配对设备和蓝牙广播。',
    emptySymptom: '列表中没有任何设备', emptyHelp: '支持 Razer（<code>1532</code>）和已接入的 Sony INZONE 接收器（<code>054C</code>）。',
    notPorted: '尚未移植的功能',
    krakenLimitation: '<strong>读取 Kraken 耳机的当前状态</strong>：Linux 内核从设备主动发送的 HID 输入报告中收集状态；本后台服务改为从本地持久化文件提供这些值。设置灯效功能可正常使用。',
    credits: 'Razer 协议知识源自 <a href="https://github.com/openrazer/openrazer">OpenRazer</a>，凝聚了 Terri Cain、Tim Theede 和众多贡献者的成果。Sony INZONE 支持移植自 penguinwokrs 的 <a href="https://github.com/penguinwokrs/openinzone">OpenInzone</a>。',
    license: '合并后的项目使用 GPL-3.0-only。OpenRazer 部分保留 GPL-2.0-or-later 声明，INZONE 移植部分保留 OpenInzone 的 GPL-3.0-only 声明。',
    trademarks: '本项目与 Razer 或 Sony 无隶属关系，也未获其认可。产品名称及商标属于各自所有者。',
    loading: '正在加载…', noMatches: '没有符合搜索条件的设备。',
    loadError: '无法加载设备列表。请查看', repository: '项目仓库',
    countAll: '{total} 款设备', countFiltered: '显示 {shown} 款，共 {total} 款设备',
    search: '搜索 {total} 款设备，例如“BlackWidow”“Viper”“0203”…',
    keyboard: '键盘', mouse: '鼠标', mousemat: '鼠标垫', keypad: '单手键盘',
    headset: '耳机', accessory: '配件', core: '外置显卡'
  };
  var en = {
    loading: 'Loading…', noMatches: 'No device matches that search.',
    loadError: 'Could not load the device list. See', repository: 'the repository',
    countAll: '{total} devices', countFiltered: '{shown} of {total} devices',
    search: 'Search {total} devices — try “BlackWidow”, “Viper”, “0203”…',
    keyboard: 'Keyboard', mouse: 'Mouse', mousemat: 'Mousepad', keypad: 'Keypad',
    headset: 'Headset', accessory: 'Accessory', core: 'eGPU'
  };
  var storageKey = 'mirage-hub.docs.language';
  var preference = 'auto';
  var language = 'en';
  var nodes = Array.from(document.querySelectorAll('[data-i18n]'));
  var labels = Array.from(document.querySelectorAll('[data-i18n-label]'));
  var images = Array.from(document.querySelectorAll('[data-src-zh]'));
  var select = document.getElementById('language-select');

  nodes.forEach(function (node) {
    en[node.dataset.i18n] = node.dataset.i18nAttr
      ? node.getAttribute(node.dataset.i18nAttr) : node.innerHTML;
  });
  labels.forEach(function (node) { en[node.dataset.i18nLabel] = node.getAttribute('aria-label'); });
  images.forEach(function (node) { node.dataset.srcEn = node.getAttribute('src'); });

  try {
    var saved = localStorage.getItem(storageKey);
    if (saved === 'en' || saved === 'zh-CN') { preference = saved; }
  } catch (_) { /* Storage can be blocked, including when opened as a local file. */ }

  function browserLanguage() {
    var languages = navigator.languages && navigator.languages.length
      ? navigator.languages : [navigator.language || 'en'];
    for (var i = 0; i < languages.length; i++) {
      var tag = languages[i].toLowerCase().split(/[-_]/)[0];
      // All Chinese locales use the available Simplified Chinese translation.
      if (tag === 'zh') { return 'zh-CN'; }
      if (tag === 'en') { return 'en'; }
    }
    return 'en';
  }

  function t(key, values) {
    var text = language === 'zh-CN' && Object.hasOwn(zh, key) ? zh[key] : en[key];
    if (text === undefined) { return key; }
    return text.replace(/\{(\w+)\}/g, function (match, name) {
      return values && Object.hasOwn(values, name) ? String(values[name]) : match;
    });
  }

  function applyLanguage() {
    language = preference === 'auto' ? browserLanguage() : preference;
    document.documentElement.lang = language;
    document.querySelector('meta[property="og:locale"]').content = language === 'zh-CN' ? 'zh_CN' : 'en_US';
    nodes.forEach(function (node) {
      if (node.dataset.i18nAttr) { node.setAttribute(node.dataset.i18nAttr, t(node.dataset.i18n)); }
      else { node.innerHTML = t(node.dataset.i18n); }
    });
    labels.forEach(function (node) { node.setAttribute('aria-label', t(node.dataset.i18nLabel)); });
    images.forEach(function (node) { node.src = language === 'zh-CN' ? node.dataset.srcZh : node.dataset.srcEn; });
    select.value = preference;
    document.dispatchEvent(new CustomEvent('mirage-hub:languagechange'));
  }

  select.addEventListener('change', function () {
    preference = select.value;
    try {
      if (preference === 'auto') { localStorage.removeItem(storageKey); }
      else { localStorage.setItem(storageKey, preference); }
    } catch (_) { /* The selection still works for the current page. */ }
    applyLanguage();
  });
  window.addEventListener('languagechange', function () {
    if (preference === 'auto') { applyLanguage(); }
  });

  window.docsI18n = { t: t, locale: function () { return language; } };
  applyLanguage();
  select.closest('.language').hidden = false;
})();
