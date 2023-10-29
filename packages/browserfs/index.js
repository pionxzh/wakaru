var BrowserFS = (() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __hasOwnProp = Object.prototype.hasOwnProperty;
  var __name = (target, value) => __defProp(target, "name", { value, configurable: true });
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __copyProps = (to, from, except, desc) => {
    if (from && typeof from === "object" || typeof from === "function") {
      for (let key of __getOwnPropNames(from))
        if (!__hasOwnProp.call(to, key) && key !== except)
          __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
    }
    return to;
  };
  var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

  // src/index.ts
  var src_exports = {};
  __export(src_exports, {
    ActionType: () => ActionType,
    ApiError: () => ApiError,
    AsyncKeyValueFile: () => AsyncKeyValueFile,
    AsyncKeyValueFileSystem: () => AsyncKeyValueFileSystem,
    BaseFile: () => BaseFile,
    BaseFileSystem: () => BaseFileSystem,
    Cred: () => Cred,
    ErrorCode: () => ErrorCode,
    ErrorStrings: () => ErrorStrings,
    FileFlag: () => FileFlag,
    FileSystem: () => FileSystem,
    FileSystemAccess: () => FileSystemAccessFileSystem,
    FileType: () => FileType,
    FolderAdapter: () => FolderAdapter,
    InMemory: () => InMemoryFileSystem,
    IndexedDB: () => IndexedDBFileSystem,
    SimpleSyncRWTransaction: () => SimpleSyncRWTransaction,
    Stats: () => Stats,
    SyncKeyValueFile: () => SyncKeyValueFile,
    SyncKeyValueFileSystem: () => SyncKeyValueFileSystem,
    SynchronousFileSystem: () => SynchronousFileSystem,
    backends: () => backends,
    configure: () => configure,
    default: () => src_default,
    fs: () => fs_default,
    getFileSystem: () => getFileSystem,
    initialize: () => initialize2,
    registerBackend: () => registerBackend
  });

  // node_modules/@jspm/core/nodelibs/browser/process.js
  var process_exports = {};
  __export(process_exports, {
    _debugEnd: () => _debugEnd,
    _debugProcess: () => _debugProcess,
    _events: () => _events,
    _eventsCount: () => _eventsCount,
    _exiting: () => _exiting,
    _fatalExceptions: () => _fatalExceptions,
    _getActiveHandles: () => _getActiveHandles,
    _getActiveRequests: () => _getActiveRequests,
    _kill: () => _kill,
    _linkedBinding: () => _linkedBinding,
    _maxListeners: () => _maxListeners,
    _preload_modules: () => _preload_modules,
    _rawDebug: () => _rawDebug,
    _startProfilerIdleNotifier: () => _startProfilerIdleNotifier,
    _stopProfilerIdleNotifier: () => _stopProfilerIdleNotifier,
    _tickCallback: () => _tickCallback,
    abort: () => abort,
    addListener: () => addListener,
    allowedNodeEnvironmentFlags: () => allowedNodeEnvironmentFlags,
    arch: () => arch,
    argv: () => argv,
    argv0: () => argv0,
    assert: () => assert,
    binding: () => binding,
    chdir: () => chdir,
    config: () => config,
    cpuUsage: () => cpuUsage,
    cwd: () => cwd,
    debugPort: () => debugPort,
    default: () => process,
    dlopen: () => dlopen,
    domain: () => domain,
    emit: () => emit,
    emitWarning: () => emitWarning,
    env: () => env,
    execArgv: () => execArgv,
    execPath: () => execPath,
    exit: () => exit,
    features: () => features,
    hasUncaughtExceptionCaptureCallback: () => hasUncaughtExceptionCaptureCallback,
    hrtime: () => hrtime,
    kill: () => kill,
    listeners: () => listeners,
    memoryUsage: () => memoryUsage,
    moduleLoadList: () => moduleLoadList,
    nextTick: () => nextTick,
    off: () => off,
    on: () => on,
    once: () => once,
    openStdin: () => openStdin,
    pid: () => pid,
    platform: () => platform,
    ppid: () => ppid,
    prependListener: () => prependListener,
    prependOnceListener: () => prependOnceListener,
    reallyExit: () => reallyExit,
    release: () => release,
    removeAllListeners: () => removeAllListeners,
    removeListener: () => removeListener,
    resourceUsage: () => resourceUsage,
    setSourceMapsEnabled: () => setSourceMapsEnabled,
    setUncaughtExceptionCaptureCallback: () => setUncaughtExceptionCaptureCallback,
    stderr: () => stderr,
    stdin: () => stdin,
    stdout: () => stdout,
    title: () => title,
    umask: () => umask,
    uptime: () => uptime,
    version: () => version,
    versions: () => versions
  });
  function unimplemented(name) {
    throw new Error("Node.js process " + name + " is not supported by JSPM core outside of Node.js");
  }
  __name(unimplemented, "unimplemented");
  var queue = [];
  var draining = false;
  var currentQueue;
  var queueIndex = -1;
  function cleanUpNextTick() {
    if (!draining || !currentQueue)
      return;
    draining = false;
    if (currentQueue.length) {
      queue = currentQueue.concat(queue);
    } else {
      queueIndex = -1;
    }
    if (queue.length)
      drainQueue();
  }
  __name(cleanUpNextTick, "cleanUpNextTick");
  function drainQueue() {
    if (draining)
      return;
    var timeout = setTimeout(cleanUpNextTick, 0);
    draining = true;
    var len = queue.length;
    while (len) {
      currentQueue = queue;
      queue = [];
      while (++queueIndex < len) {
        if (currentQueue)
          currentQueue[queueIndex].run();
      }
      queueIndex = -1;
      len = queue.length;
    }
    currentQueue = null;
    draining = false;
    clearTimeout(timeout);
  }
  __name(drainQueue, "drainQueue");
  function nextTick(fun) {
    var args = new Array(arguments.length - 1);
    if (arguments.length > 1) {
      for (var i = 1; i < arguments.length; i++)
        args[i - 1] = arguments[i];
    }
    queue.push(new Item(fun, args));
    if (queue.length === 1 && !draining)
      setTimeout(drainQueue, 0);
  }
  __name(nextTick, "nextTick");
  function Item(fun, array) {
    this.fun = fun;
    this.array = array;
  }
  __name(Item, "Item");
  Item.prototype.run = function() {
    this.fun.apply(null, this.array);
  };
  var title = "browser";
  var arch = "x64";
  var platform = "browser";
  var env = {
    PATH: "/usr/bin",
    LANG: navigator.language + ".UTF-8",
    PWD: "/",
    HOME: "/home",
    TMP: "/tmp"
  };
  var argv = ["/usr/bin/node"];
  var execArgv = [];
  var version = "v16.8.0";
  var versions = {};
  var emitWarning = /* @__PURE__ */ __name(function(message, type) {
    console.warn((type ? type + ": " : "") + message);
  }, "emitWarning");
  var binding = /* @__PURE__ */ __name(function(name) {
    unimplemented("binding");
  }, "binding");
  var umask = /* @__PURE__ */ __name(function(mask) {
    return 0;
  }, "umask");
  var cwd = /* @__PURE__ */ __name(function() {
    return "/";
  }, "cwd");
  var chdir = /* @__PURE__ */ __name(function(dir) {
  }, "chdir");
  var release = {
    name: "node",
    sourceUrl: "",
    headersUrl: "",
    libUrl: ""
  };
  function noop() {
  }
  __name(noop, "noop");
  var _rawDebug = noop;
  var moduleLoadList = [];
  function _linkedBinding(name) {
    unimplemented("_linkedBinding");
  }
  __name(_linkedBinding, "_linkedBinding");
  var domain = {};
  var _exiting = false;
  var config = {};
  function dlopen(name) {
    unimplemented("dlopen");
  }
  __name(dlopen, "dlopen");
  function _getActiveRequests() {
    return [];
  }
  __name(_getActiveRequests, "_getActiveRequests");
  function _getActiveHandles() {
    return [];
  }
  __name(_getActiveHandles, "_getActiveHandles");
  var reallyExit = noop;
  var _kill = noop;
  var cpuUsage = /* @__PURE__ */ __name(function() {
    return {};
  }, "cpuUsage");
  var resourceUsage = cpuUsage;
  var memoryUsage = cpuUsage;
  var kill = noop;
  var exit = noop;
  var openStdin = noop;
  var allowedNodeEnvironmentFlags = {};
  function assert(condition, message) {
    if (!condition)
      throw new Error(message || "assertion error");
  }
  __name(assert, "assert");
  var features = {
    inspector: false,
    debug: false,
    uv: false,
    ipv6: false,
    tls_alpn: false,
    tls_sni: false,
    tls_ocsp: false,
    tls: false,
    cached_builtins: true
  };
  var _fatalExceptions = noop;
  var setUncaughtExceptionCaptureCallback = noop;
  function hasUncaughtExceptionCaptureCallback() {
    return false;
  }
  __name(hasUncaughtExceptionCaptureCallback, "hasUncaughtExceptionCaptureCallback");
  var _tickCallback = noop;
  var _debugProcess = noop;
  var _debugEnd = noop;
  var _startProfilerIdleNotifier = noop;
  var _stopProfilerIdleNotifier = noop;
  var stdout = void 0;
  var stderr = void 0;
  var stdin = void 0;
  var abort = noop;
  var pid = 2;
  var ppid = 1;
  var execPath = "/bin/usr/node";
  var debugPort = 9229;
  var argv0 = "node";
  var _preload_modules = [];
  var setSourceMapsEnabled = noop;
  var _performance = {
    now: typeof performance !== "undefined" ? performance.now.bind(performance) : void 0,
    timing: typeof performance !== "undefined" ? performance.timing : void 0
  };
  if (_performance.now === void 0) {
    nowOffset = Date.now();
    if (_performance.timing && _performance.timing.navigationStart) {
      nowOffset = _performance.timing.navigationStart;
    }
    _performance.now = () => Date.now() - nowOffset;
  }
  var nowOffset;
  function uptime() {
    return _performance.now() / 1e3;
  }
  __name(uptime, "uptime");
  var nanoPerSec = 1e9;
  function hrtime(previousTimestamp) {
    var baseNow = Math.floor((Date.now() - _performance.now()) * 1e-3);
    var clocktime = _performance.now() * 1e-3;
    var seconds = Math.floor(clocktime) + baseNow;
    var nanoseconds = Math.floor(clocktime % 1 * 1e9);
    if (previousTimestamp) {
      seconds = seconds - previousTimestamp[0];
      nanoseconds = nanoseconds - previousTimestamp[1];
      if (nanoseconds < 0) {
        seconds--;
        nanoseconds += nanoPerSec;
      }
    }
    return [seconds, nanoseconds];
  }
  __name(hrtime, "hrtime");
  hrtime.bigint = function(time) {
    var diff = hrtime(time);
    if (typeof BigInt === "undefined") {
      return diff[0] * nanoPerSec + diff[1];
    }
    return BigInt(diff[0] * nanoPerSec) + BigInt(diff[1]);
  };
  var _maxListeners = 10;
  var _events = {};
  var _eventsCount = 0;
  function on() {
    return process;
  }
  __name(on, "on");
  var addListener = on;
  var once = on;
  var off = on;
  var removeListener = on;
  var removeAllListeners = on;
  var emit = noop;
  var prependListener = on;
  var prependOnceListener = on;
  function listeners(name) {
    return [];
  }
  __name(listeners, "listeners");
  var process = {
    version,
    versions,
    arch,
    platform,
    release,
    _rawDebug,
    moduleLoadList,
    binding,
    _linkedBinding,
    _events,
    _eventsCount,
    _maxListeners,
    on,
    addListener,
    once,
    off,
    removeListener,
    removeAllListeners,
    emit,
    prependListener,
    prependOnceListener,
    listeners,
    domain,
    _exiting,
    config,
    dlopen,
    uptime,
    _getActiveRequests,
    _getActiveHandles,
    reallyExit,
    _kill,
    cpuUsage,
    resourceUsage,
    memoryUsage,
    kill,
    exit,
    openStdin,
    allowedNodeEnvironmentFlags,
    assert,
    features,
    _fatalExceptions,
    setUncaughtExceptionCaptureCallback,
    hasUncaughtExceptionCaptureCallback,
    emitWarning,
    nextTick,
    _tickCallback,
    _debugProcess,
    _debugEnd,
    _startProfilerIdleNotifier,
    _stopProfilerIdleNotifier,
    stdout,
    stdin,
    stderr,
    abort,
    umask,
    chdir,
    cwd,
    env,
    title,
    argv,
    execArgv,
    pid,
    ppid,
    execPath,
    debugPort,
    hrtime,
    argv0,
    _preload_modules,
    setSourceMapsEnabled
  };

  // node_modules/@jspm/core/nodelibs/browser/buffer.js
  var exports$3 = {};
  var _dewExec$2 = false;
  function dew$2() {
    if (_dewExec$2)
      return exports$3;
    _dewExec$2 = true;
    exports$3.byteLength = byteLength;
    exports$3.toByteArray = toByteArray;
    exports$3.fromByteArray = fromByteArray;
    var lookup = [];
    var revLookup = [];
    var Arr = typeof Uint8Array !== "undefined" ? Uint8Array : Array;
    var code = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for (var i = 0, len = code.length; i < len; ++i) {
      lookup[i] = code[i];
      revLookup[code.charCodeAt(i)] = i;
    }
    revLookup["-".charCodeAt(0)] = 62;
    revLookup["_".charCodeAt(0)] = 63;
    function getLens(b64) {
      var len2 = b64.length;
      if (len2 % 4 > 0) {
        throw new Error("Invalid string. Length must be a multiple of 4");
      }
      var validLen = b64.indexOf("=");
      if (validLen === -1)
        validLen = len2;
      var placeHoldersLen = validLen === len2 ? 0 : 4 - validLen % 4;
      return [validLen, placeHoldersLen];
    }
    __name(getLens, "getLens");
    function byteLength(b64) {
      var lens = getLens(b64);
      var validLen = lens[0];
      var placeHoldersLen = lens[1];
      return (validLen + placeHoldersLen) * 3 / 4 - placeHoldersLen;
    }
    __name(byteLength, "byteLength");
    function _byteLength(b64, validLen, placeHoldersLen) {
      return (validLen + placeHoldersLen) * 3 / 4 - placeHoldersLen;
    }
    __name(_byteLength, "_byteLength");
    function toByteArray(b64) {
      var tmp;
      var lens = getLens(b64);
      var validLen = lens[0];
      var placeHoldersLen = lens[1];
      var arr = new Arr(_byteLength(b64, validLen, placeHoldersLen));
      var curByte = 0;
      var len2 = placeHoldersLen > 0 ? validLen - 4 : validLen;
      var i2;
      for (i2 = 0; i2 < len2; i2 += 4) {
        tmp = revLookup[b64.charCodeAt(i2)] << 18 | revLookup[b64.charCodeAt(i2 + 1)] << 12 | revLookup[b64.charCodeAt(i2 + 2)] << 6 | revLookup[b64.charCodeAt(i2 + 3)];
        arr[curByte++] = tmp >> 16 & 255;
        arr[curByte++] = tmp >> 8 & 255;
        arr[curByte++] = tmp & 255;
      }
      if (placeHoldersLen === 2) {
        tmp = revLookup[b64.charCodeAt(i2)] << 2 | revLookup[b64.charCodeAt(i2 + 1)] >> 4;
        arr[curByte++] = tmp & 255;
      }
      if (placeHoldersLen === 1) {
        tmp = revLookup[b64.charCodeAt(i2)] << 10 | revLookup[b64.charCodeAt(i2 + 1)] << 4 | revLookup[b64.charCodeAt(i2 + 2)] >> 2;
        arr[curByte++] = tmp >> 8 & 255;
        arr[curByte++] = tmp & 255;
      }
      return arr;
    }
    __name(toByteArray, "toByteArray");
    function tripletToBase64(num) {
      return lookup[num >> 18 & 63] + lookup[num >> 12 & 63] + lookup[num >> 6 & 63] + lookup[num & 63];
    }
    __name(tripletToBase64, "tripletToBase64");
    function encodeChunk(uint8, start, end) {
      var tmp;
      var output = [];
      for (var i2 = start; i2 < end; i2 += 3) {
        tmp = (uint8[i2] << 16 & 16711680) + (uint8[i2 + 1] << 8 & 65280) + (uint8[i2 + 2] & 255);
        output.push(tripletToBase64(tmp));
      }
      return output.join("");
    }
    __name(encodeChunk, "encodeChunk");
    function fromByteArray(uint8) {
      var tmp;
      var len2 = uint8.length;
      var extraBytes = len2 % 3;
      var parts = [];
      var maxChunkLength = 16383;
      for (var i2 = 0, len22 = len2 - extraBytes; i2 < len22; i2 += maxChunkLength) {
        parts.push(encodeChunk(uint8, i2, i2 + maxChunkLength > len22 ? len22 : i2 + maxChunkLength));
      }
      if (extraBytes === 1) {
        tmp = uint8[len2 - 1];
        parts.push(lookup[tmp >> 2] + lookup[tmp << 4 & 63] + "==");
      } else if (extraBytes === 2) {
        tmp = (uint8[len2 - 2] << 8) + uint8[len2 - 1];
        parts.push(lookup[tmp >> 10] + lookup[tmp >> 4 & 63] + lookup[tmp << 2 & 63] + "=");
      }
      return parts.join("");
    }
    __name(fromByteArray, "fromByteArray");
    return exports$3;
  }
  __name(dew$2, "dew$2");
  var exports$2 = {};
  var _dewExec$1 = false;
  function dew$1() {
    if (_dewExec$1)
      return exports$2;
    _dewExec$1 = true;
    exports$2.read = function(buffer, offset, isLE, mLen, nBytes) {
      var e, m;
      var eLen = nBytes * 8 - mLen - 1;
      var eMax = (1 << eLen) - 1;
      var eBias = eMax >> 1;
      var nBits = -7;
      var i = isLE ? nBytes - 1 : 0;
      var d = isLE ? -1 : 1;
      var s = buffer[offset + i];
      i += d;
      e = s & (1 << -nBits) - 1;
      s >>= -nBits;
      nBits += eLen;
      for (; nBits > 0; e = e * 256 + buffer[offset + i], i += d, nBits -= 8) {
      }
      m = e & (1 << -nBits) - 1;
      e >>= -nBits;
      nBits += mLen;
      for (; nBits > 0; m = m * 256 + buffer[offset + i], i += d, nBits -= 8) {
      }
      if (e === 0) {
        e = 1 - eBias;
      } else if (e === eMax) {
        return m ? NaN : (s ? -1 : 1) * Infinity;
      } else {
        m = m + Math.pow(2, mLen);
        e = e - eBias;
      }
      return (s ? -1 : 1) * m * Math.pow(2, e - mLen);
    };
    exports$2.write = function(buffer, value, offset, isLE, mLen, nBytes) {
      var e, m, c;
      var eLen = nBytes * 8 - mLen - 1;
      var eMax = (1 << eLen) - 1;
      var eBias = eMax >> 1;
      var rt = mLen === 23 ? Math.pow(2, -24) - Math.pow(2, -77) : 0;
      var i = isLE ? 0 : nBytes - 1;
      var d = isLE ? 1 : -1;
      var s = value < 0 || value === 0 && 1 / value < 0 ? 1 : 0;
      value = Math.abs(value);
      if (isNaN(value) || value === Infinity) {
        m = isNaN(value) ? 1 : 0;
        e = eMax;
      } else {
        e = Math.floor(Math.log(value) / Math.LN2);
        if (value * (c = Math.pow(2, -e)) < 1) {
          e--;
          c *= 2;
        }
        if (e + eBias >= 1) {
          value += rt / c;
        } else {
          value += rt * Math.pow(2, 1 - eBias);
        }
        if (value * c >= 2) {
          e++;
          c /= 2;
        }
        if (e + eBias >= eMax) {
          m = 0;
          e = eMax;
        } else if (e + eBias >= 1) {
          m = (value * c - 1) * Math.pow(2, mLen);
          e = e + eBias;
        } else {
          m = value * Math.pow(2, eBias - 1) * Math.pow(2, mLen);
          e = 0;
        }
      }
      for (; mLen >= 8; buffer[offset + i] = m & 255, i += d, m /= 256, mLen -= 8) {
      }
      e = e << mLen | m;
      eLen += mLen;
      for (; eLen > 0; buffer[offset + i] = e & 255, i += d, e /= 256, eLen -= 8) {
      }
      buffer[offset + i - d] |= s * 128;
    };
    return exports$2;
  }
  __name(dew$1, "dew$1");
  var exports$1 = {};
  var _dewExec = false;
  function dew() {
    if (_dewExec)
      return exports$1;
    _dewExec = true;
    const base64 = dew$2();
    const ieee754 = dew$1();
    const customInspectSymbol = typeof Symbol === "function" && typeof Symbol["for"] === "function" ? Symbol["for"]("nodejs.util.inspect.custom") : null;
    exports$1.Buffer = Buffer3;
    exports$1.SlowBuffer = SlowBuffer;
    exports$1.INSPECT_MAX_BYTES = 50;
    const K_MAX_LENGTH = 2147483647;
    exports$1.kMaxLength = K_MAX_LENGTH;
    Buffer3.TYPED_ARRAY_SUPPORT = typedArraySupport();
    if (!Buffer3.TYPED_ARRAY_SUPPORT && typeof console !== "undefined" && typeof console.error === "function") {
      console.error("This browser lacks typed array (Uint8Array) support which is required by `buffer` v5.x. Use `buffer` v4.x if you require old browser support.");
    }
    function typedArraySupport() {
      try {
        const arr = new Uint8Array(1);
        const proto = {
          foo: function() {
            return 42;
          }
        };
        Object.setPrototypeOf(proto, Uint8Array.prototype);
        Object.setPrototypeOf(arr, proto);
        return arr.foo() === 42;
      } catch (e) {
        return false;
      }
    }
    __name(typedArraySupport, "typedArraySupport");
    Object.defineProperty(Buffer3.prototype, "parent", {
      enumerable: true,
      get: function() {
        if (!Buffer3.isBuffer(this))
          return void 0;
        return this.buffer;
      }
    });
    Object.defineProperty(Buffer3.prototype, "offset", {
      enumerable: true,
      get: function() {
        if (!Buffer3.isBuffer(this))
          return void 0;
        return this.byteOffset;
      }
    });
    function createBuffer(length) {
      if (length > K_MAX_LENGTH) {
        throw new RangeError('The value "' + length + '" is invalid for option "size"');
      }
      const buf = new Uint8Array(length);
      Object.setPrototypeOf(buf, Buffer3.prototype);
      return buf;
    }
    __name(createBuffer, "createBuffer");
    function Buffer3(arg, encodingOrOffset, length) {
      if (typeof arg === "number") {
        if (typeof encodingOrOffset === "string") {
          throw new TypeError('The "string" argument must be of type string. Received type number');
        }
        return allocUnsafe(arg);
      }
      return from(arg, encodingOrOffset, length);
    }
    __name(Buffer3, "Buffer");
    Buffer3.poolSize = 8192;
    function from(value, encodingOrOffset, length) {
      if (typeof value === "string") {
        return fromString(value, encodingOrOffset);
      }
      if (ArrayBuffer.isView(value)) {
        return fromArrayView(value);
      }
      if (value == null) {
        throw new TypeError("The first argument must be one of type string, Buffer, ArrayBuffer, Array, or Array-like Object. Received type " + typeof value);
      }
      if (isInstance(value, ArrayBuffer) || value && isInstance(value.buffer, ArrayBuffer)) {
        return fromArrayBuffer(value, encodingOrOffset, length);
      }
      if (typeof SharedArrayBuffer !== "undefined" && (isInstance(value, SharedArrayBuffer) || value && isInstance(value.buffer, SharedArrayBuffer))) {
        return fromArrayBuffer(value, encodingOrOffset, length);
      }
      if (typeof value === "number") {
        throw new TypeError('The "value" argument must not be of type number. Received type number');
      }
      const valueOf = value.valueOf && value.valueOf();
      if (valueOf != null && valueOf !== value) {
        return Buffer3.from(valueOf, encodingOrOffset, length);
      }
      const b = fromObject(value);
      if (b)
        return b;
      if (typeof Symbol !== "undefined" && Symbol.toPrimitive != null && typeof value[Symbol.toPrimitive] === "function") {
        return Buffer3.from(value[Symbol.toPrimitive]("string"), encodingOrOffset, length);
      }
      throw new TypeError("The first argument must be one of type string, Buffer, ArrayBuffer, Array, or Array-like Object. Received type " + typeof value);
    }
    __name(from, "from");
    Buffer3.from = function(value, encodingOrOffset, length) {
      return from(value, encodingOrOffset, length);
    };
    Object.setPrototypeOf(Buffer3.prototype, Uint8Array.prototype);
    Object.setPrototypeOf(Buffer3, Uint8Array);
    function assertSize(size) {
      if (typeof size !== "number") {
        throw new TypeError('"size" argument must be of type number');
      } else if (size < 0) {
        throw new RangeError('The value "' + size + '" is invalid for option "size"');
      }
    }
    __name(assertSize, "assertSize");
    function alloc(size, fill, encoding) {
      assertSize(size);
      if (size <= 0) {
        return createBuffer(size);
      }
      if (fill !== void 0) {
        return typeof encoding === "string" ? createBuffer(size).fill(fill, encoding) : createBuffer(size).fill(fill);
      }
      return createBuffer(size);
    }
    __name(alloc, "alloc");
    Buffer3.alloc = function(size, fill, encoding) {
      return alloc(size, fill, encoding);
    };
    function allocUnsafe(size) {
      assertSize(size);
      return createBuffer(size < 0 ? 0 : checked(size) | 0);
    }
    __name(allocUnsafe, "allocUnsafe");
    Buffer3.allocUnsafe = function(size) {
      return allocUnsafe(size);
    };
    Buffer3.allocUnsafeSlow = function(size) {
      return allocUnsafe(size);
    };
    function fromString(string, encoding) {
      if (typeof encoding !== "string" || encoding === "") {
        encoding = "utf8";
      }
      if (!Buffer3.isEncoding(encoding)) {
        throw new TypeError("Unknown encoding: " + encoding);
      }
      const length = byteLength(string, encoding) | 0;
      let buf = createBuffer(length);
      const actual = buf.write(string, encoding);
      if (actual !== length) {
        buf = buf.slice(0, actual);
      }
      return buf;
    }
    __name(fromString, "fromString");
    function fromArrayLike(array) {
      const length = array.length < 0 ? 0 : checked(array.length) | 0;
      const buf = createBuffer(length);
      for (let i = 0; i < length; i += 1) {
        buf[i] = array[i] & 255;
      }
      return buf;
    }
    __name(fromArrayLike, "fromArrayLike");
    function fromArrayView(arrayView) {
      if (isInstance(arrayView, Uint8Array)) {
        const copy = new Uint8Array(arrayView);
        return fromArrayBuffer(copy.buffer, copy.byteOffset, copy.byteLength);
      }
      return fromArrayLike(arrayView);
    }
    __name(fromArrayView, "fromArrayView");
    function fromArrayBuffer(array, byteOffset, length) {
      if (byteOffset < 0 || array.byteLength < byteOffset) {
        throw new RangeError('"offset" is outside of buffer bounds');
      }
      if (array.byteLength < byteOffset + (length || 0)) {
        throw new RangeError('"length" is outside of buffer bounds');
      }
      let buf;
      if (byteOffset === void 0 && length === void 0) {
        buf = new Uint8Array(array);
      } else if (length === void 0) {
        buf = new Uint8Array(array, byteOffset);
      } else {
        buf = new Uint8Array(array, byteOffset, length);
      }
      Object.setPrototypeOf(buf, Buffer3.prototype);
      return buf;
    }
    __name(fromArrayBuffer, "fromArrayBuffer");
    function fromObject(obj) {
      if (Buffer3.isBuffer(obj)) {
        const len = checked(obj.length) | 0;
        const buf = createBuffer(len);
        if (buf.length === 0) {
          return buf;
        }
        obj.copy(buf, 0, 0, len);
        return buf;
      }
      if (obj.length !== void 0) {
        if (typeof obj.length !== "number" || numberIsNaN(obj.length)) {
          return createBuffer(0);
        }
        return fromArrayLike(obj);
      }
      if (obj.type === "Buffer" && Array.isArray(obj.data)) {
        return fromArrayLike(obj.data);
      }
    }
    __name(fromObject, "fromObject");
    function checked(length) {
      if (length >= K_MAX_LENGTH) {
        throw new RangeError("Attempt to allocate Buffer larger than maximum size: 0x" + K_MAX_LENGTH.toString(16) + " bytes");
      }
      return length | 0;
    }
    __name(checked, "checked");
    function SlowBuffer(length) {
      if (+length != length) {
        length = 0;
      }
      return Buffer3.alloc(+length);
    }
    __name(SlowBuffer, "SlowBuffer");
    Buffer3.isBuffer = /* @__PURE__ */ __name(function isBuffer(b) {
      return b != null && b._isBuffer === true && b !== Buffer3.prototype;
    }, "isBuffer");
    Buffer3.compare = /* @__PURE__ */ __name(function compare(a, b) {
      if (isInstance(a, Uint8Array))
        a = Buffer3.from(a, a.offset, a.byteLength);
      if (isInstance(b, Uint8Array))
        b = Buffer3.from(b, b.offset, b.byteLength);
      if (!Buffer3.isBuffer(a) || !Buffer3.isBuffer(b)) {
        throw new TypeError('The "buf1", "buf2" arguments must be one of type Buffer or Uint8Array');
      }
      if (a === b)
        return 0;
      let x = a.length;
      let y = b.length;
      for (let i = 0, len = Math.min(x, y); i < len; ++i) {
        if (a[i] !== b[i]) {
          x = a[i];
          y = b[i];
          break;
        }
      }
      if (x < y)
        return -1;
      if (y < x)
        return 1;
      return 0;
    }, "compare");
    Buffer3.isEncoding = /* @__PURE__ */ __name(function isEncoding(encoding) {
      switch (String(encoding).toLowerCase()) {
        case "hex":
        case "utf8":
        case "utf-8":
        case "ascii":
        case "latin1":
        case "binary":
        case "base64":
        case "ucs2":
        case "ucs-2":
        case "utf16le":
        case "utf-16le":
          return true;
        default:
          return false;
      }
    }, "isEncoding");
    Buffer3.concat = /* @__PURE__ */ __name(function concat(list, length) {
      if (!Array.isArray(list)) {
        throw new TypeError('"list" argument must be an Array of Buffers');
      }
      if (list.length === 0) {
        return Buffer3.alloc(0);
      }
      let i;
      if (length === void 0) {
        length = 0;
        for (i = 0; i < list.length; ++i) {
          length += list[i].length;
        }
      }
      const buffer = Buffer3.allocUnsafe(length);
      let pos = 0;
      for (i = 0; i < list.length; ++i) {
        let buf = list[i];
        if (isInstance(buf, Uint8Array)) {
          if (pos + buf.length > buffer.length) {
            if (!Buffer3.isBuffer(buf))
              buf = Buffer3.from(buf);
            buf.copy(buffer, pos);
          } else {
            Uint8Array.prototype.set.call(buffer, buf, pos);
          }
        } else if (!Buffer3.isBuffer(buf)) {
          throw new TypeError('"list" argument must be an Array of Buffers');
        } else {
          buf.copy(buffer, pos);
        }
        pos += buf.length;
      }
      return buffer;
    }, "concat");
    function byteLength(string, encoding) {
      if (Buffer3.isBuffer(string)) {
        return string.length;
      }
      if (ArrayBuffer.isView(string) || isInstance(string, ArrayBuffer)) {
        return string.byteLength;
      }
      if (typeof string !== "string") {
        throw new TypeError('The "string" argument must be one of type string, Buffer, or ArrayBuffer. Received type ' + typeof string);
      }
      const len = string.length;
      const mustMatch = arguments.length > 2 && arguments[2] === true;
      if (!mustMatch && len === 0)
        return 0;
      let loweredCase = false;
      for (; ; ) {
        switch (encoding) {
          case "ascii":
          case "latin1":
          case "binary":
            return len;
          case "utf8":
          case "utf-8":
            return utf8ToBytes(string).length;
          case "ucs2":
          case "ucs-2":
          case "utf16le":
          case "utf-16le":
            return len * 2;
          case "hex":
            return len >>> 1;
          case "base64":
            return base64ToBytes(string).length;
          default:
            if (loweredCase) {
              return mustMatch ? -1 : utf8ToBytes(string).length;
            }
            encoding = ("" + encoding).toLowerCase();
            loweredCase = true;
        }
      }
    }
    __name(byteLength, "byteLength");
    Buffer3.byteLength = byteLength;
    function slowToString(encoding, start, end) {
      let loweredCase = false;
      if (start === void 0 || start < 0) {
        start = 0;
      }
      if (start > this.length) {
        return "";
      }
      if (end === void 0 || end > this.length) {
        end = this.length;
      }
      if (end <= 0) {
        return "";
      }
      end >>>= 0;
      start >>>= 0;
      if (end <= start) {
        return "";
      }
      if (!encoding)
        encoding = "utf8";
      while (true) {
        switch (encoding) {
          case "hex":
            return hexSlice(this, start, end);
          case "utf8":
          case "utf-8":
            return utf8Slice(this, start, end);
          case "ascii":
            return asciiSlice(this, start, end);
          case "latin1":
          case "binary":
            return latin1Slice(this, start, end);
          case "base64":
            return base64Slice(this, start, end);
          case "ucs2":
          case "ucs-2":
          case "utf16le":
          case "utf-16le":
            return utf16leSlice(this, start, end);
          default:
            if (loweredCase)
              throw new TypeError("Unknown encoding: " + encoding);
            encoding = (encoding + "").toLowerCase();
            loweredCase = true;
        }
      }
    }
    __name(slowToString, "slowToString");
    Buffer3.prototype._isBuffer = true;
    function swap(b, n, m) {
      const i = b[n];
      b[n] = b[m];
      b[m] = i;
    }
    __name(swap, "swap");
    Buffer3.prototype.swap16 = /* @__PURE__ */ __name(function swap16() {
      const len = this.length;
      if (len % 2 !== 0) {
        throw new RangeError("Buffer size must be a multiple of 16-bits");
      }
      for (let i = 0; i < len; i += 2) {
        swap(this, i, i + 1);
      }
      return this;
    }, "swap16");
    Buffer3.prototype.swap32 = /* @__PURE__ */ __name(function swap32() {
      const len = this.length;
      if (len % 4 !== 0) {
        throw new RangeError("Buffer size must be a multiple of 32-bits");
      }
      for (let i = 0; i < len; i += 4) {
        swap(this, i, i + 3);
        swap(this, i + 1, i + 2);
      }
      return this;
    }, "swap32");
    Buffer3.prototype.swap64 = /* @__PURE__ */ __name(function swap64() {
      const len = this.length;
      if (len % 8 !== 0) {
        throw new RangeError("Buffer size must be a multiple of 64-bits");
      }
      for (let i = 0; i < len; i += 8) {
        swap(this, i, i + 7);
        swap(this, i + 1, i + 6);
        swap(this, i + 2, i + 5);
        swap(this, i + 3, i + 4);
      }
      return this;
    }, "swap64");
    Buffer3.prototype.toString = /* @__PURE__ */ __name(function toString() {
      const length = this.length;
      if (length === 0)
        return "";
      if (arguments.length === 0)
        return utf8Slice(this, 0, length);
      return slowToString.apply(this, arguments);
    }, "toString");
    Buffer3.prototype.toLocaleString = Buffer3.prototype.toString;
    Buffer3.prototype.equals = /* @__PURE__ */ __name(function equals(b) {
      if (!Buffer3.isBuffer(b))
        throw new TypeError("Argument must be a Buffer");
      if (this === b)
        return true;
      return Buffer3.compare(this, b) === 0;
    }, "equals");
    Buffer3.prototype.inspect = /* @__PURE__ */ __name(function inspect() {
      let str = "";
      const max = exports$1.INSPECT_MAX_BYTES;
      str = this.toString("hex", 0, max).replace(/(.{2})/g, "$1 ").trim();
      if (this.length > max)
        str += " ... ";
      return "<Buffer " + str + ">";
    }, "inspect");
    if (customInspectSymbol) {
      Buffer3.prototype[customInspectSymbol] = Buffer3.prototype.inspect;
    }
    Buffer3.prototype.compare = /* @__PURE__ */ __name(function compare(target, start, end, thisStart, thisEnd) {
      if (isInstance(target, Uint8Array)) {
        target = Buffer3.from(target, target.offset, target.byteLength);
      }
      if (!Buffer3.isBuffer(target)) {
        throw new TypeError('The "target" argument must be one of type Buffer or Uint8Array. Received type ' + typeof target);
      }
      if (start === void 0) {
        start = 0;
      }
      if (end === void 0) {
        end = target ? target.length : 0;
      }
      if (thisStart === void 0) {
        thisStart = 0;
      }
      if (thisEnd === void 0) {
        thisEnd = this.length;
      }
      if (start < 0 || end > target.length || thisStart < 0 || thisEnd > this.length) {
        throw new RangeError("out of range index");
      }
      if (thisStart >= thisEnd && start >= end) {
        return 0;
      }
      if (thisStart >= thisEnd) {
        return -1;
      }
      if (start >= end) {
        return 1;
      }
      start >>>= 0;
      end >>>= 0;
      thisStart >>>= 0;
      thisEnd >>>= 0;
      if (this === target)
        return 0;
      let x = thisEnd - thisStart;
      let y = end - start;
      const len = Math.min(x, y);
      const thisCopy = this.slice(thisStart, thisEnd);
      const targetCopy = target.slice(start, end);
      for (let i = 0; i < len; ++i) {
        if (thisCopy[i] !== targetCopy[i]) {
          x = thisCopy[i];
          y = targetCopy[i];
          break;
        }
      }
      if (x < y)
        return -1;
      if (y < x)
        return 1;
      return 0;
    }, "compare");
    function bidirectionalIndexOf(buffer, val, byteOffset, encoding, dir) {
      if (buffer.length === 0)
        return -1;
      if (typeof byteOffset === "string") {
        encoding = byteOffset;
        byteOffset = 0;
      } else if (byteOffset > 2147483647) {
        byteOffset = 2147483647;
      } else if (byteOffset < -2147483648) {
        byteOffset = -2147483648;
      }
      byteOffset = +byteOffset;
      if (numberIsNaN(byteOffset)) {
        byteOffset = dir ? 0 : buffer.length - 1;
      }
      if (byteOffset < 0)
        byteOffset = buffer.length + byteOffset;
      if (byteOffset >= buffer.length) {
        if (dir)
          return -1;
        else
          byteOffset = buffer.length - 1;
      } else if (byteOffset < 0) {
        if (dir)
          byteOffset = 0;
        else
          return -1;
      }
      if (typeof val === "string") {
        val = Buffer3.from(val, encoding);
      }
      if (Buffer3.isBuffer(val)) {
        if (val.length === 0) {
          return -1;
        }
        return arrayIndexOf(buffer, val, byteOffset, encoding, dir);
      } else if (typeof val === "number") {
        val = val & 255;
        if (typeof Uint8Array.prototype.indexOf === "function") {
          if (dir) {
            return Uint8Array.prototype.indexOf.call(buffer, val, byteOffset);
          } else {
            return Uint8Array.prototype.lastIndexOf.call(buffer, val, byteOffset);
          }
        }
        return arrayIndexOf(buffer, [val], byteOffset, encoding, dir);
      }
      throw new TypeError("val must be string, number or Buffer");
    }
    __name(bidirectionalIndexOf, "bidirectionalIndexOf");
    function arrayIndexOf(arr, val, byteOffset, encoding, dir) {
      let indexSize = 1;
      let arrLength = arr.length;
      let valLength = val.length;
      if (encoding !== void 0) {
        encoding = String(encoding).toLowerCase();
        if (encoding === "ucs2" || encoding === "ucs-2" || encoding === "utf16le" || encoding === "utf-16le") {
          if (arr.length < 2 || val.length < 2) {
            return -1;
          }
          indexSize = 2;
          arrLength /= 2;
          valLength /= 2;
          byteOffset /= 2;
        }
      }
      function read3(buf, i2) {
        if (indexSize === 1) {
          return buf[i2];
        } else {
          return buf.readUInt16BE(i2 * indexSize);
        }
      }
      __name(read3, "read");
      let i;
      if (dir) {
        let foundIndex = -1;
        for (i = byteOffset; i < arrLength; i++) {
          if (read3(arr, i) === read3(val, foundIndex === -1 ? 0 : i - foundIndex)) {
            if (foundIndex === -1)
              foundIndex = i;
            if (i - foundIndex + 1 === valLength)
              return foundIndex * indexSize;
          } else {
            if (foundIndex !== -1)
              i -= i - foundIndex;
            foundIndex = -1;
          }
        }
      } else {
        if (byteOffset + valLength > arrLength)
          byteOffset = arrLength - valLength;
        for (i = byteOffset; i >= 0; i--) {
          let found = true;
          for (let j = 0; j < valLength; j++) {
            if (read3(arr, i + j) !== read3(val, j)) {
              found = false;
              break;
            }
          }
          if (found)
            return i;
        }
      }
      return -1;
    }
    __name(arrayIndexOf, "arrayIndexOf");
    Buffer3.prototype.includes = /* @__PURE__ */ __name(function includes(val, byteOffset, encoding) {
      return this.indexOf(val, byteOffset, encoding) !== -1;
    }, "includes");
    Buffer3.prototype.indexOf = /* @__PURE__ */ __name(function indexOf(val, byteOffset, encoding) {
      return bidirectionalIndexOf(this, val, byteOffset, encoding, true);
    }, "indexOf");
    Buffer3.prototype.lastIndexOf = /* @__PURE__ */ __name(function lastIndexOf(val, byteOffset, encoding) {
      return bidirectionalIndexOf(this, val, byteOffset, encoding, false);
    }, "lastIndexOf");
    function hexWrite(buf, string, offset, length) {
      offset = Number(offset) || 0;
      const remaining = buf.length - offset;
      if (!length) {
        length = remaining;
      } else {
        length = Number(length);
        if (length > remaining) {
          length = remaining;
        }
      }
      const strLen = string.length;
      if (length > strLen / 2) {
        length = strLen / 2;
      }
      let i;
      for (i = 0; i < length; ++i) {
        const parsed = parseInt(string.substr(i * 2, 2), 16);
        if (numberIsNaN(parsed))
          return i;
        buf[offset + i] = parsed;
      }
      return i;
    }
    __name(hexWrite, "hexWrite");
    function utf8Write(buf, string, offset, length) {
      return blitBuffer(utf8ToBytes(string, buf.length - offset), buf, offset, length);
    }
    __name(utf8Write, "utf8Write");
    function asciiWrite(buf, string, offset, length) {
      return blitBuffer(asciiToBytes(string), buf, offset, length);
    }
    __name(asciiWrite, "asciiWrite");
    function base64Write(buf, string, offset, length) {
      return blitBuffer(base64ToBytes(string), buf, offset, length);
    }
    __name(base64Write, "base64Write");
    function ucs2Write(buf, string, offset, length) {
      return blitBuffer(utf16leToBytes(string, buf.length - offset), buf, offset, length);
    }
    __name(ucs2Write, "ucs2Write");
    Buffer3.prototype.write = /* @__PURE__ */ __name(function write3(string, offset, length, encoding) {
      if (offset === void 0) {
        encoding = "utf8";
        length = this.length;
        offset = 0;
      } else if (length === void 0 && typeof offset === "string") {
        encoding = offset;
        length = this.length;
        offset = 0;
      } else if (isFinite(offset)) {
        offset = offset >>> 0;
        if (isFinite(length)) {
          length = length >>> 0;
          if (encoding === void 0)
            encoding = "utf8";
        } else {
          encoding = length;
          length = void 0;
        }
      } else {
        throw new Error("Buffer.write(string, encoding, offset[, length]) is no longer supported");
      }
      const remaining = this.length - offset;
      if (length === void 0 || length > remaining)
        length = remaining;
      if (string.length > 0 && (length < 0 || offset < 0) || offset > this.length) {
        throw new RangeError("Attempt to write outside buffer bounds");
      }
      if (!encoding)
        encoding = "utf8";
      let loweredCase = false;
      for (; ; ) {
        switch (encoding) {
          case "hex":
            return hexWrite(this, string, offset, length);
          case "utf8":
          case "utf-8":
            return utf8Write(this, string, offset, length);
          case "ascii":
          case "latin1":
          case "binary":
            return asciiWrite(this, string, offset, length);
          case "base64":
            return base64Write(this, string, offset, length);
          case "ucs2":
          case "ucs-2":
          case "utf16le":
          case "utf-16le":
            return ucs2Write(this, string, offset, length);
          default:
            if (loweredCase)
              throw new TypeError("Unknown encoding: " + encoding);
            encoding = ("" + encoding).toLowerCase();
            loweredCase = true;
        }
      }
    }, "write");
    Buffer3.prototype.toJSON = /* @__PURE__ */ __name(function toJSON() {
      return {
        type: "Buffer",
        data: Array.prototype.slice.call(this._arr || this, 0)
      };
    }, "toJSON");
    function base64Slice(buf, start, end) {
      if (start === 0 && end === buf.length) {
        return base64.fromByteArray(buf);
      } else {
        return base64.fromByteArray(buf.slice(start, end));
      }
    }
    __name(base64Slice, "base64Slice");
    function utf8Slice(buf, start, end) {
      end = Math.min(buf.length, end);
      const res = [];
      let i = start;
      while (i < end) {
        const firstByte = buf[i];
        let codePoint = null;
        let bytesPerSequence = firstByte > 239 ? 4 : firstByte > 223 ? 3 : firstByte > 191 ? 2 : 1;
        if (i + bytesPerSequence <= end) {
          let secondByte, thirdByte, fourthByte, tempCodePoint;
          switch (bytesPerSequence) {
            case 1:
              if (firstByte < 128) {
                codePoint = firstByte;
              }
              break;
            case 2:
              secondByte = buf[i + 1];
              if ((secondByte & 192) === 128) {
                tempCodePoint = (firstByte & 31) << 6 | secondByte & 63;
                if (tempCodePoint > 127) {
                  codePoint = tempCodePoint;
                }
              }
              break;
            case 3:
              secondByte = buf[i + 1];
              thirdByte = buf[i + 2];
              if ((secondByte & 192) === 128 && (thirdByte & 192) === 128) {
                tempCodePoint = (firstByte & 15) << 12 | (secondByte & 63) << 6 | thirdByte & 63;
                if (tempCodePoint > 2047 && (tempCodePoint < 55296 || tempCodePoint > 57343)) {
                  codePoint = tempCodePoint;
                }
              }
              break;
            case 4:
              secondByte = buf[i + 1];
              thirdByte = buf[i + 2];
              fourthByte = buf[i + 3];
              if ((secondByte & 192) === 128 && (thirdByte & 192) === 128 && (fourthByte & 192) === 128) {
                tempCodePoint = (firstByte & 15) << 18 | (secondByte & 63) << 12 | (thirdByte & 63) << 6 | fourthByte & 63;
                if (tempCodePoint > 65535 && tempCodePoint < 1114112) {
                  codePoint = tempCodePoint;
                }
              }
          }
        }
        if (codePoint === null) {
          codePoint = 65533;
          bytesPerSequence = 1;
        } else if (codePoint > 65535) {
          codePoint -= 65536;
          res.push(codePoint >>> 10 & 1023 | 55296);
          codePoint = 56320 | codePoint & 1023;
        }
        res.push(codePoint);
        i += bytesPerSequence;
      }
      return decodeCodePointsArray(res);
    }
    __name(utf8Slice, "utf8Slice");
    const MAX_ARGUMENTS_LENGTH = 4096;
    function decodeCodePointsArray(codePoints) {
      const len = codePoints.length;
      if (len <= MAX_ARGUMENTS_LENGTH) {
        return String.fromCharCode.apply(String, codePoints);
      }
      let res = "";
      let i = 0;
      while (i < len) {
        res += String.fromCharCode.apply(String, codePoints.slice(i, i += MAX_ARGUMENTS_LENGTH));
      }
      return res;
    }
    __name(decodeCodePointsArray, "decodeCodePointsArray");
    function asciiSlice(buf, start, end) {
      let ret = "";
      end = Math.min(buf.length, end);
      for (let i = start; i < end; ++i) {
        ret += String.fromCharCode(buf[i] & 127);
      }
      return ret;
    }
    __name(asciiSlice, "asciiSlice");
    function latin1Slice(buf, start, end) {
      let ret = "";
      end = Math.min(buf.length, end);
      for (let i = start; i < end; ++i) {
        ret += String.fromCharCode(buf[i]);
      }
      return ret;
    }
    __name(latin1Slice, "latin1Slice");
    function hexSlice(buf, start, end) {
      const len = buf.length;
      if (!start || start < 0)
        start = 0;
      if (!end || end < 0 || end > len)
        end = len;
      let out = "";
      for (let i = start; i < end; ++i) {
        out += hexSliceLookupTable[buf[i]];
      }
      return out;
    }
    __name(hexSlice, "hexSlice");
    function utf16leSlice(buf, start, end) {
      const bytes = buf.slice(start, end);
      let res = "";
      for (let i = 0; i < bytes.length - 1; i += 2) {
        res += String.fromCharCode(bytes[i] + bytes[i + 1] * 256);
      }
      return res;
    }
    __name(utf16leSlice, "utf16leSlice");
    Buffer3.prototype.slice = /* @__PURE__ */ __name(function slice(start, end) {
      const len = this.length;
      start = ~~start;
      end = end === void 0 ? len : ~~end;
      if (start < 0) {
        start += len;
        if (start < 0)
          start = 0;
      } else if (start > len) {
        start = len;
      }
      if (end < 0) {
        end += len;
        if (end < 0)
          end = 0;
      } else if (end > len) {
        end = len;
      }
      if (end < start)
        end = start;
      const newBuf = this.subarray(start, end);
      Object.setPrototypeOf(newBuf, Buffer3.prototype);
      return newBuf;
    }, "slice");
    function checkOffset(offset, ext, length) {
      if (offset % 1 !== 0 || offset < 0)
        throw new RangeError("offset is not uint");
      if (offset + ext > length)
        throw new RangeError("Trying to access beyond buffer length");
    }
    __name(checkOffset, "checkOffset");
    Buffer3.prototype.readUintLE = Buffer3.prototype.readUIntLE = /* @__PURE__ */ __name(function readUIntLE(offset, byteLength2, noAssert) {
      offset = offset >>> 0;
      byteLength2 = byteLength2 >>> 0;
      if (!noAssert)
        checkOffset(offset, byteLength2, this.length);
      let val = this[offset];
      let mul = 1;
      let i = 0;
      while (++i < byteLength2 && (mul *= 256)) {
        val += this[offset + i] * mul;
      }
      return val;
    }, "readUIntLE");
    Buffer3.prototype.readUintBE = Buffer3.prototype.readUIntBE = /* @__PURE__ */ __name(function readUIntBE(offset, byteLength2, noAssert) {
      offset = offset >>> 0;
      byteLength2 = byteLength2 >>> 0;
      if (!noAssert) {
        checkOffset(offset, byteLength2, this.length);
      }
      let val = this[offset + --byteLength2];
      let mul = 1;
      while (byteLength2 > 0 && (mul *= 256)) {
        val += this[offset + --byteLength2] * mul;
      }
      return val;
    }, "readUIntBE");
    Buffer3.prototype.readUint8 = Buffer3.prototype.readUInt8 = /* @__PURE__ */ __name(function readUInt8(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 1, this.length);
      return this[offset];
    }, "readUInt8");
    Buffer3.prototype.readUint16LE = Buffer3.prototype.readUInt16LE = /* @__PURE__ */ __name(function readUInt16LE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 2, this.length);
      return this[offset] | this[offset + 1] << 8;
    }, "readUInt16LE");
    Buffer3.prototype.readUint16BE = Buffer3.prototype.readUInt16BE = /* @__PURE__ */ __name(function readUInt16BE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 2, this.length);
      return this[offset] << 8 | this[offset + 1];
    }, "readUInt16BE");
    Buffer3.prototype.readUint32LE = Buffer3.prototype.readUInt32LE = /* @__PURE__ */ __name(function readUInt32LE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 4, this.length);
      return (this[offset] | this[offset + 1] << 8 | this[offset + 2] << 16) + this[offset + 3] * 16777216;
    }, "readUInt32LE");
    Buffer3.prototype.readUint32BE = Buffer3.prototype.readUInt32BE = /* @__PURE__ */ __name(function readUInt32BE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 4, this.length);
      return this[offset] * 16777216 + (this[offset + 1] << 16 | this[offset + 2] << 8 | this[offset + 3]);
    }, "readUInt32BE");
    Buffer3.prototype.readBigUInt64LE = defineBigIntMethod(/* @__PURE__ */ __name(function readBigUInt64LE(offset) {
      offset = offset >>> 0;
      validateNumber(offset, "offset");
      const first = this[offset];
      const last = this[offset + 7];
      if (first === void 0 || last === void 0) {
        boundsError(offset, this.length - 8);
      }
      const lo = first + this[++offset] * 2 ** 8 + this[++offset] * 2 ** 16 + this[++offset] * 2 ** 24;
      const hi = this[++offset] + this[++offset] * 2 ** 8 + this[++offset] * 2 ** 16 + last * 2 ** 24;
      return BigInt(lo) + (BigInt(hi) << BigInt(32));
    }, "readBigUInt64LE"));
    Buffer3.prototype.readBigUInt64BE = defineBigIntMethod(/* @__PURE__ */ __name(function readBigUInt64BE(offset) {
      offset = offset >>> 0;
      validateNumber(offset, "offset");
      const first = this[offset];
      const last = this[offset + 7];
      if (first === void 0 || last === void 0) {
        boundsError(offset, this.length - 8);
      }
      const hi = first * 2 ** 24 + this[++offset] * 2 ** 16 + this[++offset] * 2 ** 8 + this[++offset];
      const lo = this[++offset] * 2 ** 24 + this[++offset] * 2 ** 16 + this[++offset] * 2 ** 8 + last;
      return (BigInt(hi) << BigInt(32)) + BigInt(lo);
    }, "readBigUInt64BE"));
    Buffer3.prototype.readIntLE = /* @__PURE__ */ __name(function readIntLE(offset, byteLength2, noAssert) {
      offset = offset >>> 0;
      byteLength2 = byteLength2 >>> 0;
      if (!noAssert)
        checkOffset(offset, byteLength2, this.length);
      let val = this[offset];
      let mul = 1;
      let i = 0;
      while (++i < byteLength2 && (mul *= 256)) {
        val += this[offset + i] * mul;
      }
      mul *= 128;
      if (val >= mul)
        val -= Math.pow(2, 8 * byteLength2);
      return val;
    }, "readIntLE");
    Buffer3.prototype.readIntBE = /* @__PURE__ */ __name(function readIntBE(offset, byteLength2, noAssert) {
      offset = offset >>> 0;
      byteLength2 = byteLength2 >>> 0;
      if (!noAssert)
        checkOffset(offset, byteLength2, this.length);
      let i = byteLength2;
      let mul = 1;
      let val = this[offset + --i];
      while (i > 0 && (mul *= 256)) {
        val += this[offset + --i] * mul;
      }
      mul *= 128;
      if (val >= mul)
        val -= Math.pow(2, 8 * byteLength2);
      return val;
    }, "readIntBE");
    Buffer3.prototype.readInt8 = /* @__PURE__ */ __name(function readInt8(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 1, this.length);
      if (!(this[offset] & 128))
        return this[offset];
      return (255 - this[offset] + 1) * -1;
    }, "readInt8");
    Buffer3.prototype.readInt16LE = /* @__PURE__ */ __name(function readInt16LE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 2, this.length);
      const val = this[offset] | this[offset + 1] << 8;
      return val & 32768 ? val | 4294901760 : val;
    }, "readInt16LE");
    Buffer3.prototype.readInt16BE = /* @__PURE__ */ __name(function readInt16BE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 2, this.length);
      const val = this[offset + 1] | this[offset] << 8;
      return val & 32768 ? val | 4294901760 : val;
    }, "readInt16BE");
    Buffer3.prototype.readInt32LE = /* @__PURE__ */ __name(function readInt32LE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 4, this.length);
      return this[offset] | this[offset + 1] << 8 | this[offset + 2] << 16 | this[offset + 3] << 24;
    }, "readInt32LE");
    Buffer3.prototype.readInt32BE = /* @__PURE__ */ __name(function readInt32BE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 4, this.length);
      return this[offset] << 24 | this[offset + 1] << 16 | this[offset + 2] << 8 | this[offset + 3];
    }, "readInt32BE");
    Buffer3.prototype.readBigInt64LE = defineBigIntMethod(/* @__PURE__ */ __name(function readBigInt64LE(offset) {
      offset = offset >>> 0;
      validateNumber(offset, "offset");
      const first = this[offset];
      const last = this[offset + 7];
      if (first === void 0 || last === void 0) {
        boundsError(offset, this.length - 8);
      }
      const val = this[offset + 4] + this[offset + 5] * 2 ** 8 + this[offset + 6] * 2 ** 16 + (last << 24);
      return (BigInt(val) << BigInt(32)) + BigInt(first + this[++offset] * 2 ** 8 + this[++offset] * 2 ** 16 + this[++offset] * 2 ** 24);
    }, "readBigInt64LE"));
    Buffer3.prototype.readBigInt64BE = defineBigIntMethod(/* @__PURE__ */ __name(function readBigInt64BE(offset) {
      offset = offset >>> 0;
      validateNumber(offset, "offset");
      const first = this[offset];
      const last = this[offset + 7];
      if (first === void 0 || last === void 0) {
        boundsError(offset, this.length - 8);
      }
      const val = (first << 24) + // Overflow
      this[++offset] * 2 ** 16 + this[++offset] * 2 ** 8 + this[++offset];
      return (BigInt(val) << BigInt(32)) + BigInt(this[++offset] * 2 ** 24 + this[++offset] * 2 ** 16 + this[++offset] * 2 ** 8 + last);
    }, "readBigInt64BE"));
    Buffer3.prototype.readFloatLE = /* @__PURE__ */ __name(function readFloatLE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 4, this.length);
      return ieee754.read(this, offset, true, 23, 4);
    }, "readFloatLE");
    Buffer3.prototype.readFloatBE = /* @__PURE__ */ __name(function readFloatBE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 4, this.length);
      return ieee754.read(this, offset, false, 23, 4);
    }, "readFloatBE");
    Buffer3.prototype.readDoubleLE = /* @__PURE__ */ __name(function readDoubleLE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 8, this.length);
      return ieee754.read(this, offset, true, 52, 8);
    }, "readDoubleLE");
    Buffer3.prototype.readDoubleBE = /* @__PURE__ */ __name(function readDoubleBE(offset, noAssert) {
      offset = offset >>> 0;
      if (!noAssert)
        checkOffset(offset, 8, this.length);
      return ieee754.read(this, offset, false, 52, 8);
    }, "readDoubleBE");
    function checkInt(buf, value, offset, ext, max, min) {
      if (!Buffer3.isBuffer(buf))
        throw new TypeError('"buffer" argument must be a Buffer instance');
      if (value > max || value < min)
        throw new RangeError('"value" argument is out of bounds');
      if (offset + ext > buf.length)
        throw new RangeError("Index out of range");
    }
    __name(checkInt, "checkInt");
    Buffer3.prototype.writeUintLE = Buffer3.prototype.writeUIntLE = /* @__PURE__ */ __name(function writeUIntLE(value, offset, byteLength2, noAssert) {
      value = +value;
      offset = offset >>> 0;
      byteLength2 = byteLength2 >>> 0;
      if (!noAssert) {
        const maxBytes = Math.pow(2, 8 * byteLength2) - 1;
        checkInt(this, value, offset, byteLength2, maxBytes, 0);
      }
      let mul = 1;
      let i = 0;
      this[offset] = value & 255;
      while (++i < byteLength2 && (mul *= 256)) {
        this[offset + i] = value / mul & 255;
      }
      return offset + byteLength2;
    }, "writeUIntLE");
    Buffer3.prototype.writeUintBE = Buffer3.prototype.writeUIntBE = /* @__PURE__ */ __name(function writeUIntBE(value, offset, byteLength2, noAssert) {
      value = +value;
      offset = offset >>> 0;
      byteLength2 = byteLength2 >>> 0;
      if (!noAssert) {
        const maxBytes = Math.pow(2, 8 * byteLength2) - 1;
        checkInt(this, value, offset, byteLength2, maxBytes, 0);
      }
      let i = byteLength2 - 1;
      let mul = 1;
      this[offset + i] = value & 255;
      while (--i >= 0 && (mul *= 256)) {
        this[offset + i] = value / mul & 255;
      }
      return offset + byteLength2;
    }, "writeUIntBE");
    Buffer3.prototype.writeUint8 = Buffer3.prototype.writeUInt8 = /* @__PURE__ */ __name(function writeUInt8(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 1, 255, 0);
      this[offset] = value & 255;
      return offset + 1;
    }, "writeUInt8");
    Buffer3.prototype.writeUint16LE = Buffer3.prototype.writeUInt16LE = /* @__PURE__ */ __name(function writeUInt16LE(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 2, 65535, 0);
      this[offset] = value & 255;
      this[offset + 1] = value >>> 8;
      return offset + 2;
    }, "writeUInt16LE");
    Buffer3.prototype.writeUint16BE = Buffer3.prototype.writeUInt16BE = /* @__PURE__ */ __name(function writeUInt16BE(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 2, 65535, 0);
      this[offset] = value >>> 8;
      this[offset + 1] = value & 255;
      return offset + 2;
    }, "writeUInt16BE");
    Buffer3.prototype.writeUint32LE = Buffer3.prototype.writeUInt32LE = /* @__PURE__ */ __name(function writeUInt32LE(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 4, 4294967295, 0);
      this[offset + 3] = value >>> 24;
      this[offset + 2] = value >>> 16;
      this[offset + 1] = value >>> 8;
      this[offset] = value & 255;
      return offset + 4;
    }, "writeUInt32LE");
    Buffer3.prototype.writeUint32BE = Buffer3.prototype.writeUInt32BE = /* @__PURE__ */ __name(function writeUInt32BE(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 4, 4294967295, 0);
      this[offset] = value >>> 24;
      this[offset + 1] = value >>> 16;
      this[offset + 2] = value >>> 8;
      this[offset + 3] = value & 255;
      return offset + 4;
    }, "writeUInt32BE");
    function wrtBigUInt64LE(buf, value, offset, min, max) {
      checkIntBI(value, min, max, buf, offset, 7);
      let lo = Number(value & BigInt(4294967295));
      buf[offset++] = lo;
      lo = lo >> 8;
      buf[offset++] = lo;
      lo = lo >> 8;
      buf[offset++] = lo;
      lo = lo >> 8;
      buf[offset++] = lo;
      let hi = Number(value >> BigInt(32) & BigInt(4294967295));
      buf[offset++] = hi;
      hi = hi >> 8;
      buf[offset++] = hi;
      hi = hi >> 8;
      buf[offset++] = hi;
      hi = hi >> 8;
      buf[offset++] = hi;
      return offset;
    }
    __name(wrtBigUInt64LE, "wrtBigUInt64LE");
    function wrtBigUInt64BE(buf, value, offset, min, max) {
      checkIntBI(value, min, max, buf, offset, 7);
      let lo = Number(value & BigInt(4294967295));
      buf[offset + 7] = lo;
      lo = lo >> 8;
      buf[offset + 6] = lo;
      lo = lo >> 8;
      buf[offset + 5] = lo;
      lo = lo >> 8;
      buf[offset + 4] = lo;
      let hi = Number(value >> BigInt(32) & BigInt(4294967295));
      buf[offset + 3] = hi;
      hi = hi >> 8;
      buf[offset + 2] = hi;
      hi = hi >> 8;
      buf[offset + 1] = hi;
      hi = hi >> 8;
      buf[offset] = hi;
      return offset + 8;
    }
    __name(wrtBigUInt64BE, "wrtBigUInt64BE");
    Buffer3.prototype.writeBigUInt64LE = defineBigIntMethod(/* @__PURE__ */ __name(function writeBigUInt64LE(value, offset = 0) {
      return wrtBigUInt64LE(this, value, offset, BigInt(0), BigInt("0xffffffffffffffff"));
    }, "writeBigUInt64LE"));
    Buffer3.prototype.writeBigUInt64BE = defineBigIntMethod(/* @__PURE__ */ __name(function writeBigUInt64BE(value, offset = 0) {
      return wrtBigUInt64BE(this, value, offset, BigInt(0), BigInt("0xffffffffffffffff"));
    }, "writeBigUInt64BE"));
    Buffer3.prototype.writeIntLE = /* @__PURE__ */ __name(function writeIntLE(value, offset, byteLength2, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert) {
        const limit = Math.pow(2, 8 * byteLength2 - 1);
        checkInt(this, value, offset, byteLength2, limit - 1, -limit);
      }
      let i = 0;
      let mul = 1;
      let sub = 0;
      this[offset] = value & 255;
      while (++i < byteLength2 && (mul *= 256)) {
        if (value < 0 && sub === 0 && this[offset + i - 1] !== 0) {
          sub = 1;
        }
        this[offset + i] = (value / mul >> 0) - sub & 255;
      }
      return offset + byteLength2;
    }, "writeIntLE");
    Buffer3.prototype.writeIntBE = /* @__PURE__ */ __name(function writeIntBE(value, offset, byteLength2, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert) {
        const limit = Math.pow(2, 8 * byteLength2 - 1);
        checkInt(this, value, offset, byteLength2, limit - 1, -limit);
      }
      let i = byteLength2 - 1;
      let mul = 1;
      let sub = 0;
      this[offset + i] = value & 255;
      while (--i >= 0 && (mul *= 256)) {
        if (value < 0 && sub === 0 && this[offset + i + 1] !== 0) {
          sub = 1;
        }
        this[offset + i] = (value / mul >> 0) - sub & 255;
      }
      return offset + byteLength2;
    }, "writeIntBE");
    Buffer3.prototype.writeInt8 = /* @__PURE__ */ __name(function writeInt8(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 1, 127, -128);
      if (value < 0)
        value = 255 + value + 1;
      this[offset] = value & 255;
      return offset + 1;
    }, "writeInt8");
    Buffer3.prototype.writeInt16LE = /* @__PURE__ */ __name(function writeInt16LE(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 2, 32767, -32768);
      this[offset] = value & 255;
      this[offset + 1] = value >>> 8;
      return offset + 2;
    }, "writeInt16LE");
    Buffer3.prototype.writeInt16BE = /* @__PURE__ */ __name(function writeInt16BE(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 2, 32767, -32768);
      this[offset] = value >>> 8;
      this[offset + 1] = value & 255;
      return offset + 2;
    }, "writeInt16BE");
    Buffer3.prototype.writeInt32LE = /* @__PURE__ */ __name(function writeInt32LE(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 4, 2147483647, -2147483648);
      this[offset] = value & 255;
      this[offset + 1] = value >>> 8;
      this[offset + 2] = value >>> 16;
      this[offset + 3] = value >>> 24;
      return offset + 4;
    }, "writeInt32LE");
    Buffer3.prototype.writeInt32BE = /* @__PURE__ */ __name(function writeInt32BE(value, offset, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert)
        checkInt(this, value, offset, 4, 2147483647, -2147483648);
      if (value < 0)
        value = 4294967295 + value + 1;
      this[offset] = value >>> 24;
      this[offset + 1] = value >>> 16;
      this[offset + 2] = value >>> 8;
      this[offset + 3] = value & 255;
      return offset + 4;
    }, "writeInt32BE");
    Buffer3.prototype.writeBigInt64LE = defineBigIntMethod(/* @__PURE__ */ __name(function writeBigInt64LE(value, offset = 0) {
      return wrtBigUInt64LE(this, value, offset, -BigInt("0x8000000000000000"), BigInt("0x7fffffffffffffff"));
    }, "writeBigInt64LE"));
    Buffer3.prototype.writeBigInt64BE = defineBigIntMethod(/* @__PURE__ */ __name(function writeBigInt64BE(value, offset = 0) {
      return wrtBigUInt64BE(this, value, offset, -BigInt("0x8000000000000000"), BigInt("0x7fffffffffffffff"));
    }, "writeBigInt64BE"));
    function checkIEEE754(buf, value, offset, ext, max, min) {
      if (offset + ext > buf.length)
        throw new RangeError("Index out of range");
      if (offset < 0)
        throw new RangeError("Index out of range");
    }
    __name(checkIEEE754, "checkIEEE754");
    function writeFloat(buf, value, offset, littleEndian, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert) {
        checkIEEE754(buf, value, offset, 4);
      }
      ieee754.write(buf, value, offset, littleEndian, 23, 4);
      return offset + 4;
    }
    __name(writeFloat, "writeFloat");
    Buffer3.prototype.writeFloatLE = /* @__PURE__ */ __name(function writeFloatLE(value, offset, noAssert) {
      return writeFloat(this, value, offset, true, noAssert);
    }, "writeFloatLE");
    Buffer3.prototype.writeFloatBE = /* @__PURE__ */ __name(function writeFloatBE(value, offset, noAssert) {
      return writeFloat(this, value, offset, false, noAssert);
    }, "writeFloatBE");
    function writeDouble(buf, value, offset, littleEndian, noAssert) {
      value = +value;
      offset = offset >>> 0;
      if (!noAssert) {
        checkIEEE754(buf, value, offset, 8);
      }
      ieee754.write(buf, value, offset, littleEndian, 52, 8);
      return offset + 8;
    }
    __name(writeDouble, "writeDouble");
    Buffer3.prototype.writeDoubleLE = /* @__PURE__ */ __name(function writeDoubleLE(value, offset, noAssert) {
      return writeDouble(this, value, offset, true, noAssert);
    }, "writeDoubleLE");
    Buffer3.prototype.writeDoubleBE = /* @__PURE__ */ __name(function writeDoubleBE(value, offset, noAssert) {
      return writeDouble(this, value, offset, false, noAssert);
    }, "writeDoubleBE");
    Buffer3.prototype.copy = /* @__PURE__ */ __name(function copy(target, targetStart, start, end) {
      if (!Buffer3.isBuffer(target))
        throw new TypeError("argument should be a Buffer");
      if (!start)
        start = 0;
      if (!end && end !== 0)
        end = this.length;
      if (targetStart >= target.length)
        targetStart = target.length;
      if (!targetStart)
        targetStart = 0;
      if (end > 0 && end < start)
        end = start;
      if (end === start)
        return 0;
      if (target.length === 0 || this.length === 0)
        return 0;
      if (targetStart < 0) {
        throw new RangeError("targetStart out of bounds");
      }
      if (start < 0 || start >= this.length)
        throw new RangeError("Index out of range");
      if (end < 0)
        throw new RangeError("sourceEnd out of bounds");
      if (end > this.length)
        end = this.length;
      if (target.length - targetStart < end - start) {
        end = target.length - targetStart + start;
      }
      const len = end - start;
      if (this === target && typeof Uint8Array.prototype.copyWithin === "function") {
        this.copyWithin(targetStart, start, end);
      } else {
        Uint8Array.prototype.set.call(target, this.subarray(start, end), targetStart);
      }
      return len;
    }, "copy");
    Buffer3.prototype.fill = /* @__PURE__ */ __name(function fill(val, start, end, encoding) {
      if (typeof val === "string") {
        if (typeof start === "string") {
          encoding = start;
          start = 0;
          end = this.length;
        } else if (typeof end === "string") {
          encoding = end;
          end = this.length;
        }
        if (encoding !== void 0 && typeof encoding !== "string") {
          throw new TypeError("encoding must be a string");
        }
        if (typeof encoding === "string" && !Buffer3.isEncoding(encoding)) {
          throw new TypeError("Unknown encoding: " + encoding);
        }
        if (val.length === 1) {
          const code = val.charCodeAt(0);
          if (encoding === "utf8" && code < 128 || encoding === "latin1") {
            val = code;
          }
        }
      } else if (typeof val === "number") {
        val = val & 255;
      } else if (typeof val === "boolean") {
        val = Number(val);
      }
      if (start < 0 || this.length < start || this.length < end) {
        throw new RangeError("Out of range index");
      }
      if (end <= start) {
        return this;
      }
      start = start >>> 0;
      end = end === void 0 ? this.length : end >>> 0;
      if (!val)
        val = 0;
      let i;
      if (typeof val === "number") {
        for (i = start; i < end; ++i) {
          this[i] = val;
        }
      } else {
        const bytes = Buffer3.isBuffer(val) ? val : Buffer3.from(val, encoding);
        const len = bytes.length;
        if (len === 0) {
          throw new TypeError('The value "' + val + '" is invalid for argument "value"');
        }
        for (i = 0; i < end - start; ++i) {
          this[i + start] = bytes[i % len];
        }
      }
      return this;
    }, "fill");
    const errors = {};
    function E(sym, getMessage, Base) {
      errors[sym] = /* @__PURE__ */ __name(class NodeError extends Base {
        constructor() {
          super();
          Object.defineProperty(this, "message", {
            value: getMessage.apply(this, arguments),
            writable: true,
            configurable: true
          });
          this.name = `${this.name} [${sym}]`;
          this.stack;
          delete this.name;
        }
        get code() {
          return sym;
        }
        set code(value) {
          Object.defineProperty(this, "code", {
            configurable: true,
            enumerable: true,
            value,
            writable: true
          });
        }
        toString() {
          return `${this.name} [${sym}]: ${this.message}`;
        }
      }, "NodeError");
    }
    __name(E, "E");
    E("ERR_BUFFER_OUT_OF_BOUNDS", function(name) {
      if (name) {
        return `${name} is outside of buffer bounds`;
      }
      return "Attempt to access memory outside buffer bounds";
    }, RangeError);
    E("ERR_INVALID_ARG_TYPE", function(name, actual) {
      return `The "${name}" argument must be of type number. Received type ${typeof actual}`;
    }, TypeError);
    E("ERR_OUT_OF_RANGE", function(str, range, input) {
      let msg = `The value of "${str}" is out of range.`;
      let received = input;
      if (Number.isInteger(input) && Math.abs(input) > 2 ** 32) {
        received = addNumericalSeparator(String(input));
      } else if (typeof input === "bigint") {
        received = String(input);
        if (input > BigInt(2) ** BigInt(32) || input < -(BigInt(2) ** BigInt(32))) {
          received = addNumericalSeparator(received);
        }
        received += "n";
      }
      msg += ` It must be ${range}. Received ${received}`;
      return msg;
    }, RangeError);
    function addNumericalSeparator(val) {
      let res = "";
      let i = val.length;
      const start = val[0] === "-" ? 1 : 0;
      for (; i >= start + 4; i -= 3) {
        res = `_${val.slice(i - 3, i)}${res}`;
      }
      return `${val.slice(0, i)}${res}`;
    }
    __name(addNumericalSeparator, "addNumericalSeparator");
    function checkBounds(buf, offset, byteLength2) {
      validateNumber(offset, "offset");
      if (buf[offset] === void 0 || buf[offset + byteLength2] === void 0) {
        boundsError(offset, buf.length - (byteLength2 + 1));
      }
    }
    __name(checkBounds, "checkBounds");
    function checkIntBI(value, min, max, buf, offset, byteLength2) {
      if (value > max || value < min) {
        const n = typeof min === "bigint" ? "n" : "";
        let range;
        if (byteLength2 > 3) {
          if (min === 0 || min === BigInt(0)) {
            range = `>= 0${n} and < 2${n} ** ${(byteLength2 + 1) * 8}${n}`;
          } else {
            range = `>= -(2${n} ** ${(byteLength2 + 1) * 8 - 1}${n}) and < 2 ** ${(byteLength2 + 1) * 8 - 1}${n}`;
          }
        } else {
          range = `>= ${min}${n} and <= ${max}${n}`;
        }
        throw new errors.ERR_OUT_OF_RANGE("value", range, value);
      }
      checkBounds(buf, offset, byteLength2);
    }
    __name(checkIntBI, "checkIntBI");
    function validateNumber(value, name) {
      if (typeof value !== "number") {
        throw new errors.ERR_INVALID_ARG_TYPE(name, "number", value);
      }
    }
    __name(validateNumber, "validateNumber");
    function boundsError(value, length, type) {
      if (Math.floor(value) !== value) {
        validateNumber(value, type);
        throw new errors.ERR_OUT_OF_RANGE(type || "offset", "an integer", value);
      }
      if (length < 0) {
        throw new errors.ERR_BUFFER_OUT_OF_BOUNDS();
      }
      throw new errors.ERR_OUT_OF_RANGE(type || "offset", `>= ${type ? 1 : 0} and <= ${length}`, value);
    }
    __name(boundsError, "boundsError");
    const INVALID_BASE64_RE = /[^+/0-9A-Za-z-_]/g;
    function base64clean(str) {
      str = str.split("=")[0];
      str = str.trim().replace(INVALID_BASE64_RE, "");
      if (str.length < 2)
        return "";
      while (str.length % 4 !== 0) {
        str = str + "=";
      }
      return str;
    }
    __name(base64clean, "base64clean");
    function utf8ToBytes(string, units) {
      units = units || Infinity;
      let codePoint;
      const length = string.length;
      let leadSurrogate = null;
      const bytes = [];
      for (let i = 0; i < length; ++i) {
        codePoint = string.charCodeAt(i);
        if (codePoint > 55295 && codePoint < 57344) {
          if (!leadSurrogate) {
            if (codePoint > 56319) {
              if ((units -= 3) > -1)
                bytes.push(239, 191, 189);
              continue;
            } else if (i + 1 === length) {
              if ((units -= 3) > -1)
                bytes.push(239, 191, 189);
              continue;
            }
            leadSurrogate = codePoint;
            continue;
          }
          if (codePoint < 56320) {
            if ((units -= 3) > -1)
              bytes.push(239, 191, 189);
            leadSurrogate = codePoint;
            continue;
          }
          codePoint = (leadSurrogate - 55296 << 10 | codePoint - 56320) + 65536;
        } else if (leadSurrogate) {
          if ((units -= 3) > -1)
            bytes.push(239, 191, 189);
        }
        leadSurrogate = null;
        if (codePoint < 128) {
          if ((units -= 1) < 0)
            break;
          bytes.push(codePoint);
        } else if (codePoint < 2048) {
          if ((units -= 2) < 0)
            break;
          bytes.push(codePoint >> 6 | 192, codePoint & 63 | 128);
        } else if (codePoint < 65536) {
          if ((units -= 3) < 0)
            break;
          bytes.push(codePoint >> 12 | 224, codePoint >> 6 & 63 | 128, codePoint & 63 | 128);
        } else if (codePoint < 1114112) {
          if ((units -= 4) < 0)
            break;
          bytes.push(codePoint >> 18 | 240, codePoint >> 12 & 63 | 128, codePoint >> 6 & 63 | 128, codePoint & 63 | 128);
        } else {
          throw new Error("Invalid code point");
        }
      }
      return bytes;
    }
    __name(utf8ToBytes, "utf8ToBytes");
    function asciiToBytes(str) {
      const byteArray = [];
      for (let i = 0; i < str.length; ++i) {
        byteArray.push(str.charCodeAt(i) & 255);
      }
      return byteArray;
    }
    __name(asciiToBytes, "asciiToBytes");
    function utf16leToBytes(str, units) {
      let c, hi, lo;
      const byteArray = [];
      for (let i = 0; i < str.length; ++i) {
        if ((units -= 2) < 0)
          break;
        c = str.charCodeAt(i);
        hi = c >> 8;
        lo = c % 256;
        byteArray.push(lo);
        byteArray.push(hi);
      }
      return byteArray;
    }
    __name(utf16leToBytes, "utf16leToBytes");
    function base64ToBytes(str) {
      return base64.toByteArray(base64clean(str));
    }
    __name(base64ToBytes, "base64ToBytes");
    function blitBuffer(src, dst, offset, length) {
      let i;
      for (i = 0; i < length; ++i) {
        if (i + offset >= dst.length || i >= src.length)
          break;
        dst[i + offset] = src[i];
      }
      return i;
    }
    __name(blitBuffer, "blitBuffer");
    function isInstance(obj, type) {
      return obj instanceof type || obj != null && obj.constructor != null && obj.constructor.name != null && obj.constructor.name === type.name;
    }
    __name(isInstance, "isInstance");
    function numberIsNaN(obj) {
      return obj !== obj;
    }
    __name(numberIsNaN, "numberIsNaN");
    const hexSliceLookupTable = function() {
      const alphabet = "0123456789abcdef";
      const table = new Array(256);
      for (let i = 0; i < 16; ++i) {
        const i16 = i * 16;
        for (let j = 0; j < 16; ++j) {
          table[i16 + j] = alphabet[i] + alphabet[j];
        }
      }
      return table;
    }();
    function defineBigIntMethod(fn) {
      return typeof BigInt === "undefined" ? BufferBigIntNotDefined : fn;
    }
    __name(defineBigIntMethod, "defineBigIntMethod");
    function BufferBigIntNotDefined() {
      throw new Error("BigInt not supported");
    }
    __name(BufferBigIntNotDefined, "BufferBigIntNotDefined");
    return exports$1;
  }
  __name(dew, "dew");
  var exports = dew();
  exports["Buffer"];
  exports["SlowBuffer"];
  exports["INSPECT_MAX_BYTES"];
  exports["kMaxLength"];
  var Buffer2 = exports.Buffer;
  var INSPECT_MAX_BYTES = exports.INSPECT_MAX_BYTES;
  var kMaxLength = exports.kMaxLength;

  // src/emulation/index.ts
  var emulation_exports = {};
  __export(emulation_exports, {
    _toUnixTimestamp: () => _toUnixTimestamp,
    access: () => access2,
    accessSync: () => accessSync,
    appendFile: () => appendFile2,
    appendFileSync: () => appendFileSync,
    chmod: () => chmod2,
    chmodSync: () => chmodSync,
    chown: () => chown2,
    chownSync: () => chownSync,
    close: () => close2,
    closeSync: () => closeSync,
    constants: () => constants_exports,
    createReadStream: () => createReadStream2,
    createWriteStream: () => createWriteStream2,
    exists: () => exists2,
    existsSync: () => existsSync,
    fchmod: () => fchmod2,
    fchmodSync: () => fchmodSync,
    fchown: () => fchown2,
    fchownSync: () => fchownSync,
    fdatasync: () => fdatasync2,
    fdatasyncSync: () => fdatasyncSync,
    fstat: () => fstat2,
    fstatSync: () => fstatSync,
    fsync: () => fsync2,
    fsyncSync: () => fsyncSync,
    ftruncate: () => ftruncate2,
    ftruncateSync: () => ftruncateSync,
    futimes: () => futimes2,
    futimesSync: () => futimesSync,
    getMount: () => getMount,
    getMounts: () => getMounts,
    initialize: () => initialize,
    lchmod: () => lchmod2,
    lchmodSync: () => lchmodSync,
    lchown: () => lchown2,
    lchownSync: () => lchownSync,
    link: () => link2,
    linkSync: () => linkSync,
    lstat: () => lstat2,
    lstatSync: () => lstatSync,
    lutimes: () => lutimes2,
    lutimesSync: () => lutimesSync,
    mkdir: () => mkdir2,
    mkdirSync: () => mkdirSync,
    mount: () => mount,
    open: () => open2,
    openSync: () => openSync,
    promises: () => promises_exports,
    read: () => read2,
    readFile: () => readFile2,
    readFileSync: () => readFileSync,
    readSync: () => readSync,
    readdir: () => readdir2,
    readdirSync: () => readdirSync,
    readlink: () => readlink2,
    readlinkSync: () => readlinkSync,
    realpath: () => realpath2,
    realpathSync: () => realpathSync,
    rename: () => rename2,
    renameSync: () => renameSync,
    rmdir: () => rmdir2,
    rmdirSync: () => rmdirSync,
    stat: () => stat2,
    statSync: () => statSync,
    symlink: () => symlink2,
    symlinkSync: () => symlinkSync,
    truncate: () => truncate2,
    truncateSync: () => truncateSync,
    umount: () => umount,
    unlink: () => unlink2,
    unlinkSync: () => unlinkSync,
    unwatchFile: () => unwatchFile2,
    utimes: () => utimes2,
    utimesSync: () => utimesSync,
    watch: () => watch2,
    watchFile: () => watchFile2,
    write: () => write2,
    writeFile: () => writeFile2,
    writeFileSync: () => writeFileSync,
    writeSync: () => writeSync
  });

  // src/ApiError.ts
  var ErrorCode = /* @__PURE__ */ ((ErrorCode2) => {
    ErrorCode2[ErrorCode2["EPERM"] = 1] = "EPERM";
    ErrorCode2[ErrorCode2["ENOENT"] = 2] = "ENOENT";
    ErrorCode2[ErrorCode2["EIO"] = 5] = "EIO";
    ErrorCode2[ErrorCode2["EBADF"] = 9] = "EBADF";
    ErrorCode2[ErrorCode2["EACCES"] = 13] = "EACCES";
    ErrorCode2[ErrorCode2["EBUSY"] = 16] = "EBUSY";
    ErrorCode2[ErrorCode2["EEXIST"] = 17] = "EEXIST";
    ErrorCode2[ErrorCode2["ENOTDIR"] = 20] = "ENOTDIR";
    ErrorCode2[ErrorCode2["EISDIR"] = 21] = "EISDIR";
    ErrorCode2[ErrorCode2["EINVAL"] = 22] = "EINVAL";
    ErrorCode2[ErrorCode2["EFBIG"] = 27] = "EFBIG";
    ErrorCode2[ErrorCode2["ENOSPC"] = 28] = "ENOSPC";
    ErrorCode2[ErrorCode2["EROFS"] = 30] = "EROFS";
    ErrorCode2[ErrorCode2["ENOTEMPTY"] = 39] = "ENOTEMPTY";
    ErrorCode2[ErrorCode2["ENOTSUP"] = 95] = "ENOTSUP";
    return ErrorCode2;
  })(ErrorCode || {});
  var ErrorStrings = {};
  ErrorStrings[1 /* EPERM */] = "Operation not permitted.";
  ErrorStrings[2 /* ENOENT */] = "No such file or directory.";
  ErrorStrings[5 /* EIO */] = "Input/output error.";
  ErrorStrings[9 /* EBADF */] = "Bad file descriptor.";
  ErrorStrings[13 /* EACCES */] = "Permission denied.";
  ErrorStrings[16 /* EBUSY */] = "Resource busy or locked.";
  ErrorStrings[17 /* EEXIST */] = "File exists.";
  ErrorStrings[20 /* ENOTDIR */] = "File is not a directory.";
  ErrorStrings[21 /* EISDIR */] = "File is a directory.";
  ErrorStrings[22 /* EINVAL */] = "Invalid argument.";
  ErrorStrings[27 /* EFBIG */] = "File is too big.";
  ErrorStrings[28 /* ENOSPC */] = "No space left on disk.";
  ErrorStrings[30 /* EROFS */] = "Cannot modify a read-only file system.";
  ErrorStrings[39 /* ENOTEMPTY */] = "Directory is not empty.";
  ErrorStrings[95 /* ENOTSUP */] = "Operation is not supported.";
  var ApiError = class extends Error {
    /**
     * Represents a BrowserFS error. Passed back to applications after a failed
     * call to the BrowserFS API.
     *
     * Error codes mirror those returned by regular Unix file operations, which is
     * what Node returns.
     * @constructor ApiError
     * @param type The type of the error.
     * @param [message] A descriptive error message.
     */
    constructor(type, message = ErrorStrings[type], path) {
      super(message);
      // Unsupported.
      this.syscall = "";
      this.errno = type;
      this.code = ErrorCode[type];
      this.path = path;
      this.message = `Error: ${this.code}: ${message}${this.path ? `, '${this.path}'` : ""}`;
    }
    static fromJSON(json) {
      const err = new ApiError(json.errno, json.message, json.path);
      err.code = json.code;
      err.stack = json.stack;
      return err;
    }
    /**
     * Creates an ApiError object from a buffer.
     */
    static fromBuffer(buffer, i = 0) {
      return ApiError.fromJSON(JSON.parse(buffer.toString("utf8", i + 4, i + 4 + buffer.readUInt32LE(i))));
    }
    static FileError(code, p) {
      return new ApiError(code, ErrorStrings[code], p);
    }
    static EACCES(path) {
      return this.FileError(13 /* EACCES */, path);
    }
    static ENOENT(path) {
      return this.FileError(2 /* ENOENT */, path);
    }
    static EEXIST(path) {
      return this.FileError(17 /* EEXIST */, path);
    }
    static EISDIR(path) {
      return this.FileError(21 /* EISDIR */, path);
    }
    static ENOTDIR(path) {
      return this.FileError(20 /* ENOTDIR */, path);
    }
    static EPERM(path) {
      return this.FileError(1 /* EPERM */, path);
    }
    static ENOTEMPTY(path) {
      return this.FileError(39 /* ENOTEMPTY */, path);
    }
    /**
     * @return A friendly error message.
     */
    toString() {
      return this.message;
    }
    toJSON() {
      return {
        errno: this.errno,
        code: this.code,
        path: this.path,
        stack: this.stack,
        message: this.message
      };
    }
    /**
     * Writes the API error into a buffer.
     */
    writeToBuffer(buffer = Buffer2.alloc(this.bufferSize()), i = 0) {
      const bytesWritten = buffer.write(JSON.stringify(this.toJSON()), i + 4);
      buffer.writeUInt32LE(bytesWritten, i);
      return buffer;
    }
    /**
     * The size of the API error in buffer-form in bytes.
     */
    bufferSize() {
      return 4 + Buffer2.byteLength(JSON.stringify(this.toJSON()));
    }
  };
  __name(ApiError, "ApiError");

  // node_modules/@jspm/core/nodelibs/browser/chunk-2eac56ff.js
  var exports2 = {};
  var _dewExec2 = false;
  var _global = typeof globalThis !== "undefined" ? globalThis : typeof self !== "undefined" ? self : global;
  function dew2() {
    if (_dewExec2)
      return exports2;
    _dewExec2 = true;
    var process3 = exports2 = {};
    var cachedSetTimeout;
    var cachedClearTimeout;
    function defaultSetTimout() {
      throw new Error("setTimeout has not been defined");
    }
    __name(defaultSetTimout, "defaultSetTimout");
    function defaultClearTimeout() {
      throw new Error("clearTimeout has not been defined");
    }
    __name(defaultClearTimeout, "defaultClearTimeout");
    (function() {
      try {
        if (typeof setTimeout === "function") {
          cachedSetTimeout = setTimeout;
        } else {
          cachedSetTimeout = defaultSetTimout;
        }
      } catch (e) {
        cachedSetTimeout = defaultSetTimout;
      }
      try {
        if (typeof clearTimeout === "function") {
          cachedClearTimeout = clearTimeout;
        } else {
          cachedClearTimeout = defaultClearTimeout;
        }
      } catch (e) {
        cachedClearTimeout = defaultClearTimeout;
      }
    })();
    function runTimeout(fun) {
      if (cachedSetTimeout === setTimeout) {
        return setTimeout(fun, 0);
      }
      if ((cachedSetTimeout === defaultSetTimout || !cachedSetTimeout) && setTimeout) {
        cachedSetTimeout = setTimeout;
        return setTimeout(fun, 0);
      }
      try {
        return cachedSetTimeout(fun, 0);
      } catch (e) {
        try {
          return cachedSetTimeout.call(null, fun, 0);
        } catch (e2) {
          return cachedSetTimeout.call(this || _global, fun, 0);
        }
      }
    }
    __name(runTimeout, "runTimeout");
    function runClearTimeout(marker) {
      if (cachedClearTimeout === clearTimeout) {
        return clearTimeout(marker);
      }
      if ((cachedClearTimeout === defaultClearTimeout || !cachedClearTimeout) && clearTimeout) {
        cachedClearTimeout = clearTimeout;
        return clearTimeout(marker);
      }
      try {
        return cachedClearTimeout(marker);
      } catch (e) {
        try {
          return cachedClearTimeout.call(null, marker);
        } catch (e2) {
          return cachedClearTimeout.call(this || _global, marker);
        }
      }
    }
    __name(runClearTimeout, "runClearTimeout");
    var queue2 = [];
    var draining2 = false;
    var currentQueue2;
    var queueIndex2 = -1;
    function cleanUpNextTick2() {
      if (!draining2 || !currentQueue2) {
        return;
      }
      draining2 = false;
      if (currentQueue2.length) {
        queue2 = currentQueue2.concat(queue2);
      } else {
        queueIndex2 = -1;
      }
      if (queue2.length) {
        drainQueue2();
      }
    }
    __name(cleanUpNextTick2, "cleanUpNextTick");
    function drainQueue2() {
      if (draining2) {
        return;
      }
      var timeout = runTimeout(cleanUpNextTick2);
      draining2 = true;
      var len = queue2.length;
      while (len) {
        currentQueue2 = queue2;
        queue2 = [];
        while (++queueIndex2 < len) {
          if (currentQueue2) {
            currentQueue2[queueIndex2].run();
          }
        }
        queueIndex2 = -1;
        len = queue2.length;
      }
      currentQueue2 = null;
      draining2 = false;
      runClearTimeout(timeout);
    }
    __name(drainQueue2, "drainQueue");
    process3.nextTick = function(fun) {
      var args = new Array(arguments.length - 1);
      if (arguments.length > 1) {
        for (var i = 1; i < arguments.length; i++) {
          args[i - 1] = arguments[i];
        }
      }
      queue2.push(new Item2(fun, args));
      if (queue2.length === 1 && !draining2) {
        runTimeout(drainQueue2);
      }
    };
    function Item2(fun, array) {
      (this || _global).fun = fun;
      (this || _global).array = array;
    }
    __name(Item2, "Item");
    Item2.prototype.run = function() {
      (this || _global).fun.apply(null, (this || _global).array);
    };
    process3.title = "browser";
    process3.browser = true;
    process3.env = {};
    process3.argv = [];
    process3.version = "";
    process3.versions = {};
    function noop2() {
    }
    __name(noop2, "noop");
    process3.on = noop2;
    process3.addListener = noop2;
    process3.once = noop2;
    process3.off = noop2;
    process3.removeListener = noop2;
    process3.removeAllListeners = noop2;
    process3.emit = noop2;
    process3.prependListener = noop2;
    process3.prependOnceListener = noop2;
    process3.listeners = function(name) {
      return [];
    };
    process3.binding = function(name) {
      throw new Error("process.binding is not supported");
    };
    process3.cwd = function() {
      return "/";
    };
    process3.chdir = function(dir) {
      throw new Error("process.chdir is not supported");
    };
    process3.umask = function() {
      return 0;
    };
    return exports2;
  }
  __name(dew2, "dew");
  var process2 = dew2();
  process2.platform = "browser";
  process2.addListener;
  process2.argv;
  process2.binding;
  process2.browser;
  process2.chdir;
  process2.cwd;
  process2.emit;
  process2.env;
  process2.listeners;
  process2.nextTick;
  process2.off;
  process2.on;
  process2.once;
  process2.prependListener;
  process2.prependOnceListener;
  process2.removeAllListeners;
  process2.removeListener;
  process2.title;
  process2.umask;
  process2.version;
  process2.versions;

  // node_modules/@jspm/core/nodelibs/browser/chunk-23dbec7b.js
  var exports$12 = {};
  var _dewExec3 = false;
  function dew3() {
    if (_dewExec3)
      return exports$12;
    _dewExec3 = true;
    var process$1 = process2;
    function assertPath(path) {
      if (typeof path !== "string") {
        throw new TypeError("Path must be a string. Received " + JSON.stringify(path));
      }
    }
    __name(assertPath, "assertPath");
    function normalizeStringPosix(path, allowAboveRoot) {
      var res = "";
      var lastSegmentLength = 0;
      var lastSlash = -1;
      var dots = 0;
      var code;
      for (var i = 0; i <= path.length; ++i) {
        if (i < path.length)
          code = path.charCodeAt(i);
        else if (code === 47)
          break;
        else
          code = 47;
        if (code === 47) {
          if (lastSlash === i - 1 || dots === 1)
            ;
          else if (lastSlash !== i - 1 && dots === 2) {
            if (res.length < 2 || lastSegmentLength !== 2 || res.charCodeAt(res.length - 1) !== 46 || res.charCodeAt(res.length - 2) !== 46) {
              if (res.length > 2) {
                var lastSlashIndex = res.lastIndexOf("/");
                if (lastSlashIndex !== res.length - 1) {
                  if (lastSlashIndex === -1) {
                    res = "";
                    lastSegmentLength = 0;
                  } else {
                    res = res.slice(0, lastSlashIndex);
                    lastSegmentLength = res.length - 1 - res.lastIndexOf("/");
                  }
                  lastSlash = i;
                  dots = 0;
                  continue;
                }
              } else if (res.length === 2 || res.length === 1) {
                res = "";
                lastSegmentLength = 0;
                lastSlash = i;
                dots = 0;
                continue;
              }
            }
            if (allowAboveRoot) {
              if (res.length > 0)
                res += "/..";
              else
                res = "..";
              lastSegmentLength = 2;
            }
          } else {
            if (res.length > 0)
              res += "/" + path.slice(lastSlash + 1, i);
            else
              res = path.slice(lastSlash + 1, i);
            lastSegmentLength = i - lastSlash - 1;
          }
          lastSlash = i;
          dots = 0;
        } else if (code === 46 && dots !== -1) {
          ++dots;
        } else {
          dots = -1;
        }
      }
      return res;
    }
    __name(normalizeStringPosix, "normalizeStringPosix");
    function _format(sep2, pathObject) {
      var dir = pathObject.dir || pathObject.root;
      var base = pathObject.base || (pathObject.name || "") + (pathObject.ext || "");
      if (!dir) {
        return base;
      }
      if (dir === pathObject.root) {
        return dir + base;
      }
      return dir + sep2 + base;
    }
    __name(_format, "_format");
    var posix2 = {
      // path.resolve([from ...], to)
      resolve: /* @__PURE__ */ __name(function resolve2() {
        var resolvedPath = "";
        var resolvedAbsolute = false;
        var cwd2;
        for (var i = arguments.length - 1; i >= -1 && !resolvedAbsolute; i--) {
          var path;
          if (i >= 0)
            path = arguments[i];
          else {
            if (cwd2 === void 0)
              cwd2 = process$1.cwd();
            path = cwd2;
          }
          assertPath(path);
          if (path.length === 0) {
            continue;
          }
          resolvedPath = path + "/" + resolvedPath;
          resolvedAbsolute = path.charCodeAt(0) === 47;
        }
        resolvedPath = normalizeStringPosix(resolvedPath, !resolvedAbsolute);
        if (resolvedAbsolute) {
          if (resolvedPath.length > 0)
            return "/" + resolvedPath;
          else
            return "/";
        } else if (resolvedPath.length > 0) {
          return resolvedPath;
        } else {
          return ".";
        }
      }, "resolve"),
      normalize: /* @__PURE__ */ __name(function normalize2(path) {
        assertPath(path);
        if (path.length === 0)
          return ".";
        var isAbsolute2 = path.charCodeAt(0) === 47;
        var trailingSeparator = path.charCodeAt(path.length - 1) === 47;
        path = normalizeStringPosix(path, !isAbsolute2);
        if (path.length === 0 && !isAbsolute2)
          path = ".";
        if (path.length > 0 && trailingSeparator)
          path += "/";
        if (isAbsolute2)
          return "/" + path;
        return path;
      }, "normalize"),
      isAbsolute: /* @__PURE__ */ __name(function isAbsolute2(path) {
        assertPath(path);
        return path.length > 0 && path.charCodeAt(0) === 47;
      }, "isAbsolute"),
      join: /* @__PURE__ */ __name(function join2() {
        if (arguments.length === 0)
          return ".";
        var joined;
        for (var i = 0; i < arguments.length; ++i) {
          var arg = arguments[i];
          assertPath(arg);
          if (arg.length > 0) {
            if (joined === void 0)
              joined = arg;
            else
              joined += "/" + arg;
          }
        }
        if (joined === void 0)
          return ".";
        return posix2.normalize(joined);
      }, "join"),
      relative: /* @__PURE__ */ __name(function relative2(from, to) {
        assertPath(from);
        assertPath(to);
        if (from === to)
          return "";
        from = posix2.resolve(from);
        to = posix2.resolve(to);
        if (from === to)
          return "";
        var fromStart = 1;
        for (; fromStart < from.length; ++fromStart) {
          if (from.charCodeAt(fromStart) !== 47)
            break;
        }
        var fromEnd = from.length;
        var fromLen = fromEnd - fromStart;
        var toStart = 1;
        for (; toStart < to.length; ++toStart) {
          if (to.charCodeAt(toStart) !== 47)
            break;
        }
        var toEnd = to.length;
        var toLen = toEnd - toStart;
        var length = fromLen < toLen ? fromLen : toLen;
        var lastCommonSep = -1;
        var i = 0;
        for (; i <= length; ++i) {
          if (i === length) {
            if (toLen > length) {
              if (to.charCodeAt(toStart + i) === 47) {
                return to.slice(toStart + i + 1);
              } else if (i === 0) {
                return to.slice(toStart + i);
              }
            } else if (fromLen > length) {
              if (from.charCodeAt(fromStart + i) === 47) {
                lastCommonSep = i;
              } else if (i === 0) {
                lastCommonSep = 0;
              }
            }
            break;
          }
          var fromCode = from.charCodeAt(fromStart + i);
          var toCode = to.charCodeAt(toStart + i);
          if (fromCode !== toCode)
            break;
          else if (fromCode === 47)
            lastCommonSep = i;
        }
        var out = "";
        for (i = fromStart + lastCommonSep + 1; i <= fromEnd; ++i) {
          if (i === fromEnd || from.charCodeAt(i) === 47) {
            if (out.length === 0)
              out += "..";
            else
              out += "/..";
          }
        }
        if (out.length > 0)
          return out + to.slice(toStart + lastCommonSep);
        else {
          toStart += lastCommonSep;
          if (to.charCodeAt(toStart) === 47)
            ++toStart;
          return to.slice(toStart);
        }
      }, "relative"),
      _makeLong: /* @__PURE__ */ __name(function _makeLong2(path) {
        return path;
      }, "_makeLong"),
      dirname: /* @__PURE__ */ __name(function dirname2(path) {
        assertPath(path);
        if (path.length === 0)
          return ".";
        var code = path.charCodeAt(0);
        var hasRoot = code === 47;
        var end = -1;
        var matchedSlash = true;
        for (var i = path.length - 1; i >= 1; --i) {
          code = path.charCodeAt(i);
          if (code === 47) {
            if (!matchedSlash) {
              end = i;
              break;
            }
          } else {
            matchedSlash = false;
          }
        }
        if (end === -1)
          return hasRoot ? "/" : ".";
        if (hasRoot && end === 1)
          return "//";
        return path.slice(0, end);
      }, "dirname"),
      basename: /* @__PURE__ */ __name(function basename2(path, ext) {
        if (ext !== void 0 && typeof ext !== "string")
          throw new TypeError('"ext" argument must be a string');
        assertPath(path);
        var start = 0;
        var end = -1;
        var matchedSlash = true;
        var i;
        if (ext !== void 0 && ext.length > 0 && ext.length <= path.length) {
          if (ext.length === path.length && ext === path)
            return "";
          var extIdx = ext.length - 1;
          var firstNonSlashEnd = -1;
          for (i = path.length - 1; i >= 0; --i) {
            var code = path.charCodeAt(i);
            if (code === 47) {
              if (!matchedSlash) {
                start = i + 1;
                break;
              }
            } else {
              if (firstNonSlashEnd === -1) {
                matchedSlash = false;
                firstNonSlashEnd = i + 1;
              }
              if (extIdx >= 0) {
                if (code === ext.charCodeAt(extIdx)) {
                  if (--extIdx === -1) {
                    end = i;
                  }
                } else {
                  extIdx = -1;
                  end = firstNonSlashEnd;
                }
              }
            }
          }
          if (start === end)
            end = firstNonSlashEnd;
          else if (end === -1)
            end = path.length;
          return path.slice(start, end);
        } else {
          for (i = path.length - 1; i >= 0; --i) {
            if (path.charCodeAt(i) === 47) {
              if (!matchedSlash) {
                start = i + 1;
                break;
              }
            } else if (end === -1) {
              matchedSlash = false;
              end = i + 1;
            }
          }
          if (end === -1)
            return "";
          return path.slice(start, end);
        }
      }, "basename"),
      extname: /* @__PURE__ */ __name(function extname2(path) {
        assertPath(path);
        var startDot = -1;
        var startPart = 0;
        var end = -1;
        var matchedSlash = true;
        var preDotState = 0;
        for (var i = path.length - 1; i >= 0; --i) {
          var code = path.charCodeAt(i);
          if (code === 47) {
            if (!matchedSlash) {
              startPart = i + 1;
              break;
            }
            continue;
          }
          if (end === -1) {
            matchedSlash = false;
            end = i + 1;
          }
          if (code === 46) {
            if (startDot === -1)
              startDot = i;
            else if (preDotState !== 1)
              preDotState = 1;
          } else if (startDot !== -1) {
            preDotState = -1;
          }
        }
        if (startDot === -1 || end === -1 || // We saw a non-dot character immediately before the dot
        preDotState === 0 || // The (right-most) trimmed path component is exactly '..'
        preDotState === 1 && startDot === end - 1 && startDot === startPart + 1) {
          return "";
        }
        return path.slice(startDot, end);
      }, "extname"),
      format: /* @__PURE__ */ __name(function format2(pathObject) {
        if (pathObject === null || typeof pathObject !== "object") {
          throw new TypeError('The "pathObject" argument must be of type Object. Received type ' + typeof pathObject);
        }
        return _format("/", pathObject);
      }, "format"),
      parse: /* @__PURE__ */ __name(function parse2(path) {
        assertPath(path);
        var ret = {
          root: "",
          dir: "",
          base: "",
          ext: "",
          name: ""
        };
        if (path.length === 0)
          return ret;
        var code = path.charCodeAt(0);
        var isAbsolute2 = code === 47;
        var start;
        if (isAbsolute2) {
          ret.root = "/";
          start = 1;
        } else {
          start = 0;
        }
        var startDot = -1;
        var startPart = 0;
        var end = -1;
        var matchedSlash = true;
        var i = path.length - 1;
        var preDotState = 0;
        for (; i >= start; --i) {
          code = path.charCodeAt(i);
          if (code === 47) {
            if (!matchedSlash) {
              startPart = i + 1;
              break;
            }
            continue;
          }
          if (end === -1) {
            matchedSlash = false;
            end = i + 1;
          }
          if (code === 46) {
            if (startDot === -1)
              startDot = i;
            else if (preDotState !== 1)
              preDotState = 1;
          } else if (startDot !== -1) {
            preDotState = -1;
          }
        }
        if (startDot === -1 || end === -1 || // We saw a non-dot character immediately before the dot
        preDotState === 0 || // The (right-most) trimmed path component is exactly '..'
        preDotState === 1 && startDot === end - 1 && startDot === startPart + 1) {
          if (end !== -1) {
            if (startPart === 0 && isAbsolute2)
              ret.base = ret.name = path.slice(1, end);
            else
              ret.base = ret.name = path.slice(startPart, end);
          }
        } else {
          if (startPart === 0 && isAbsolute2) {
            ret.name = path.slice(1, startDot);
            ret.base = path.slice(1, end);
          } else {
            ret.name = path.slice(startPart, startDot);
            ret.base = path.slice(startPart, end);
          }
          ret.ext = path.slice(startDot, end);
        }
        if (startPart > 0)
          ret.dir = path.slice(0, startPart - 1);
        else if (isAbsolute2)
          ret.dir = "/";
        return ret;
      }, "parse"),
      sep: "/",
      delimiter: ":",
      win32: null,
      posix: null
    };
    posix2.posix = posix2;
    exports$12 = posix2;
    return exports$12;
  }
  __name(dew3, "dew");
  var exports3 = dew3();

  // node_modules/@jspm/core/nodelibs/browser/path.js
  var _makeLong = exports3._makeLong;
  var basename = exports3.basename;
  var delimiter = exports3.delimiter;
  var dirname = exports3.dirname;
  var extname = exports3.extname;
  var format = exports3.format;
  var isAbsolute = exports3.isAbsolute;
  var join = exports3.join;
  var normalize = exports3.normalize;
  var parse = exports3.parse;
  var posix = exports3.posix;
  var relative = exports3.relative;
  var resolve = exports3.resolve;
  var sep = exports3.sep;
  var win32 = exports3.win32;

  // src/file.ts
  var ActionType = /* @__PURE__ */ ((ActionType2) => {
    ActionType2[ActionType2["NOP"] = 0] = "NOP";
    ActionType2[ActionType2["THROW_EXCEPTION"] = 1] = "THROW_EXCEPTION";
    ActionType2[ActionType2["TRUNCATE_FILE"] = 2] = "TRUNCATE_FILE";
    ActionType2[ActionType2["CREATE_FILE"] = 3] = "CREATE_FILE";
    return ActionType2;
  })(ActionType || {});
  var _FileFlag = class {
    /**
     * Get an object representing the given file flag.
     * @param modeStr The string representing the flag
     * @return The FileFlag object representing the flag
     * @throw when the flag string is invalid
     */
    static getFileFlag(flagStr) {
      if (!_FileFlag.flagCache.has(flagStr)) {
        _FileFlag.flagCache.set(flagStr, new _FileFlag(flagStr));
      }
      return _FileFlag.flagCache.get(flagStr);
    }
    /**
     * This should never be called directly.
     * @param modeStr The string representing the mode
     * @throw when the mode string is invalid
     */
    constructor(flagStr) {
      this.flagStr = flagStr;
      if (_FileFlag.validFlagStrs.indexOf(flagStr) < 0) {
        throw new ApiError(22 /* EINVAL */, "Invalid flag: " + flagStr);
      }
    }
    /**
     * Get the underlying flag string for this flag.
     */
    getFlagString() {
      return this.flagStr;
    }
    /**
     * Get the equivalent mode (0b0xxx: read, write, execute)
     * Note: Execute will always be 0
     */
    getMode() {
      let mode = 0;
      mode <<= 1;
      mode += +this.isReadable();
      mode <<= 1;
      mode += +this.isWriteable();
      mode <<= 1;
      return mode;
    }
    /**
     * Returns true if the file is readable.
     */
    isReadable() {
      return this.flagStr.indexOf("r") !== -1 || this.flagStr.indexOf("+") !== -1;
    }
    /**
     * Returns true if the file is writeable.
     */
    isWriteable() {
      return this.flagStr.indexOf("w") !== -1 || this.flagStr.indexOf("a") !== -1 || this.flagStr.indexOf("+") !== -1;
    }
    /**
     * Returns true if the file mode should truncate.
     */
    isTruncating() {
      return this.flagStr.indexOf("w") !== -1;
    }
    /**
     * Returns true if the file is appendable.
     */
    isAppendable() {
      return this.flagStr.indexOf("a") !== -1;
    }
    /**
     * Returns true if the file is open in synchronous mode.
     */
    isSynchronous() {
      return this.flagStr.indexOf("s") !== -1;
    }
    /**
     * Returns true if the file is open in exclusive mode.
     */
    isExclusive() {
      return this.flagStr.indexOf("x") !== -1;
    }
    /**
     * Returns one of the static fields on this object that indicates the
     * appropriate response to the path existing.
     */
    pathExistsAction() {
      if (this.isExclusive()) {
        return 1 /* THROW_EXCEPTION */;
      } else if (this.isTruncating()) {
        return 2 /* TRUNCATE_FILE */;
      } else {
        return 0 /* NOP */;
      }
    }
    /**
     * Returns one of the static fields on this object that indicates the
     * appropriate response to the path not existing.
     */
    pathNotExistsAction() {
      if ((this.isWriteable() || this.isAppendable()) && this.flagStr !== "r+") {
        return 3 /* CREATE_FILE */;
      } else {
        return 1 /* THROW_EXCEPTION */;
      }
    }
  };
  var FileFlag = _FileFlag;
  __name(FileFlag, "FileFlag");
  // Contains cached FileMode instances.
  FileFlag.flagCache = /* @__PURE__ */ new Map();
  // Array of valid mode strings.
  FileFlag.validFlagStrs = ["r", "r+", "rs", "rs+", "w", "wx", "w+", "wx+", "a", "ax", "a+", "ax+"];
  var BaseFile = class {
    async sync() {
      throw new ApiError(95 /* ENOTSUP */);
    }
    syncSync() {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async datasync() {
      return this.sync();
    }
    datasyncSync() {
      return this.syncSync();
    }
    async chown(uid, gid) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    chownSync(uid, gid) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async chmod(mode) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    chmodSync(mode) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async utimes(atime, mtime) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    utimesSync(atime, mtime) {
      throw new ApiError(95 /* ENOTSUP */);
    }
  };
  __name(BaseFile, "BaseFile");

  // src/filesystem.ts
  var FileSystem = class {
    constructor(options) {
    }
  };
  __name(FileSystem, "FileSystem");
  var _BaseFileSystem = class extends FileSystem {
    constructor(options) {
      super();
      this._ready = Promise.resolve(this);
    }
    get metadata() {
      return {
        name: this.constructor.name,
        readonly: false,
        synchronous: false,
        supportsProperties: false,
        supportsLinks: false,
        totalSpace: 0,
        freeSpace: 0
      };
    }
    whenReady() {
      return this._ready;
    }
    /**
     * Opens the file at path p with the given flag. The file must exist.
     * @param p The path to open.
     * @param flag The flag to use when opening the file.
     */
    async openFile(p, flag, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    /**
     * Create the file at path p with the given mode. Then, open it with the given
     * flag.
     */
    async createFile(p, flag, mode, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async open(p, flag, mode, cred2) {
      try {
        const stats = await this.stat(p, cred2);
        switch (flag.pathExistsAction()) {
          case 1 /* THROW_EXCEPTION */:
            throw ApiError.EEXIST(p);
          case 2 /* TRUNCATE_FILE */:
            const fd = await this.openFile(p, flag, cred2);
            if (!fd)
              throw new Error("BFS has reached an impossible code path; please file a bug.");
            await fd.truncate(0);
            await fd.sync();
            return fd;
          case 0 /* NOP */:
            return this.openFile(p, flag, cred2);
          default:
            throw new ApiError(22 /* EINVAL */, "Invalid FileFlag object.");
        }
      } catch (e) {
        switch (flag.pathNotExistsAction()) {
          case 3 /* CREATE_FILE */:
            const parentStats = await this.stat(dirname(p), cred2);
            if (parentStats && !parentStats.isDirectory()) {
              throw ApiError.ENOTDIR(dirname(p));
            }
            return this.createFile(p, flag, mode, cred2);
          case 1 /* THROW_EXCEPTION */:
            throw ApiError.ENOENT(p);
          default:
            throw new ApiError(22 /* EINVAL */, "Invalid FileFlag object.");
        }
      }
    }
    async access(p, mode, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    accessSync(p, mode, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async rename(oldPath, newPath, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    renameSync(oldPath, newPath, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async stat(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    statSync(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    /**
     * Opens the file at path p with the given flag. The file must exist.
     * @param p The path to open.
     * @param flag The flag to use when opening the file.
     * @return A File object corresponding to the opened file.
     */
    openFileSync(p, flag, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    /**
     * Create the file at path p with the given mode. Then, open it with the given
     * flag.
     */
    createFileSync(p, flag, mode, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    openSync(p, flag, mode, cred2) {
      let stats;
      try {
        stats = this.statSync(p, cred2);
      } catch (e) {
        switch (flag.pathNotExistsAction()) {
          case 3 /* CREATE_FILE */:
            const parentStats = this.statSync(dirname(p), cred2);
            if (!parentStats.isDirectory()) {
              throw ApiError.ENOTDIR(dirname(p));
            }
            return this.createFileSync(p, flag, mode, cred2);
          case 1 /* THROW_EXCEPTION */:
            throw ApiError.ENOENT(p);
          default:
            throw new ApiError(22 /* EINVAL */, "Invalid FileFlag object.");
        }
      }
      if (!stats.hasAccess(mode, cred2)) {
        throw ApiError.EACCES(p);
      }
      switch (flag.pathExistsAction()) {
        case 1 /* THROW_EXCEPTION */:
          throw ApiError.EEXIST(p);
        case 2 /* TRUNCATE_FILE */:
          this.unlinkSync(p, cred2);
          return this.createFileSync(p, flag, stats.mode, cred2);
        case 0 /* NOP */:
          return this.openFileSync(p, flag, cred2);
        default:
          throw new ApiError(22 /* EINVAL */, "Invalid FileFlag object.");
      }
    }
    async unlink(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    unlinkSync(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async rmdir(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    rmdirSync(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async mkdir(p, mode, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    mkdirSync(p, mode, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async readdir(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    readdirSync(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async exists(p, cred2) {
      try {
        await this.stat(p, cred2);
        return true;
      } catch (e) {
        return false;
      }
    }
    existsSync(p, cred2) {
      try {
        this.statSync(p, cred2);
        return true;
      } catch (e) {
        return false;
      }
    }
    async realpath(p, cred2) {
      if (this.metadata.supportsLinks) {
        const splitPath = p.split(sep);
        for (let i = 0; i < splitPath.length; i++) {
          const addPaths = splitPath.slice(0, i + 1);
          splitPath[i] = join(...addPaths);
        }
        return splitPath.join(sep);
      } else {
        if (!await this.exists(p, cred2)) {
          throw ApiError.ENOENT(p);
        }
        return p;
      }
    }
    realpathSync(p, cred2) {
      if (this.metadata.supportsLinks) {
        const splitPath = p.split(sep);
        for (let i = 0; i < splitPath.length; i++) {
          const addPaths = splitPath.slice(0, i + 1);
          splitPath[i] = join(...addPaths);
        }
        return splitPath.join(sep);
      } else {
        if (this.existsSync(p, cred2)) {
          return p;
        } else {
          throw ApiError.ENOENT(p);
        }
      }
    }
    async truncate(p, len, cred2) {
      const fd = await this.open(p, FileFlag.getFileFlag("r+"), 420, cred2);
      try {
        await fd.truncate(len);
      } finally {
        await fd.close();
      }
    }
    truncateSync(p, len, cred2) {
      const fd = this.openSync(p, FileFlag.getFileFlag("r+"), 420, cred2);
      try {
        fd.truncateSync(len);
      } finally {
        fd.closeSync();
      }
    }
    async readFile(fname, encoding, flag, cred2) {
      const fd = await this.open(fname, flag, 420, cred2);
      try {
        const stat3 = await fd.stat();
        const buf = Buffer2.alloc(stat3.size);
        await fd.read(buf, 0, stat3.size, 0);
        await fd.close();
        if (encoding === null) {
          return buf;
        }
        return buf.toString(encoding);
      } finally {
        await fd.close();
      }
    }
    readFileSync(fname, encoding, flag, cred2) {
      const fd = this.openSync(fname, flag, 420, cred2);
      try {
        const stat3 = fd.statSync();
        const buf = Buffer2.alloc(stat3.size);
        fd.readSync(buf, 0, stat3.size, 0);
        fd.closeSync();
        if (encoding === null) {
          return buf;
        }
        return buf.toString(encoding);
      } finally {
        fd.closeSync();
      }
    }
    async writeFile(fname, data, encoding, flag, mode, cred2) {
      const fd = await this.open(fname, flag, mode, cred2);
      try {
        if (typeof data === "string") {
          data = Buffer2.from(data, encoding);
        }
        await fd.write(data, 0, data.length, 0);
      } finally {
        await fd.close();
      }
    }
    writeFileSync(fname, data, encoding, flag, mode, cred2) {
      const fd = this.openSync(fname, flag, mode, cred2);
      try {
        if (typeof data === "string") {
          data = Buffer2.from(data, encoding);
        }
        fd.writeSync(data, 0, data.length, 0);
      } finally {
        fd.closeSync();
      }
    }
    async appendFile(fname, data, encoding, flag, mode, cred2) {
      const fd = await this.open(fname, flag, mode, cred2);
      try {
        if (typeof data === "string") {
          data = Buffer2.from(data, encoding);
        }
        await fd.write(data, 0, data.length, null);
      } finally {
        await fd.close();
      }
    }
    appendFileSync(fname, data, encoding, flag, mode, cred2) {
      const fd = this.openSync(fname, flag, mode, cred2);
      try {
        if (typeof data === "string") {
          data = Buffer2.from(data, encoding);
        }
        fd.writeSync(data, 0, data.length, null);
      } finally {
        fd.closeSync();
      }
    }
    async chmod(p, mode, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    chmodSync(p, mode, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async chown(p, new_uid, new_gid, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    chownSync(p, new_uid, new_gid, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async utimes(p, atime, mtime, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    utimesSync(p, atime, mtime, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async link(srcpath, dstpath, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    linkSync(srcpath, dstpath, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async symlink(srcpath, dstpath, type, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    symlinkSync(srcpath, dstpath, type, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    async readlink(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
    readlinkSync(p, cred2) {
      throw new ApiError(95 /* ENOTSUP */);
    }
  };
  var BaseFileSystem = _BaseFileSystem;
  __name(BaseFileSystem, "BaseFileSystem");
  BaseFileSystem.Name = _BaseFileSystem.name;
  var SynchronousFileSystem = class extends BaseFileSystem {
    get metadata() {
      return { ...super.metadata, synchronous: true };
    }
    async access(p, mode, cred2) {
      return this.accessSync(p, mode, cred2);
    }
    async rename(oldPath, newPath, cred2) {
      return this.renameSync(oldPath, newPath, cred2);
    }
    async stat(p, cred2) {
      return this.statSync(p, cred2);
    }
    async open(p, flags, mode, cred2) {
      return this.openSync(p, flags, mode, cred2);
    }
    async unlink(p, cred2) {
      return this.unlinkSync(p, cred2);
    }
    async rmdir(p, cred2) {
      return this.rmdirSync(p, cred2);
    }
    async mkdir(p, mode, cred2) {
      return this.mkdirSync(p, mode, cred2);
    }
    async readdir(p, cred2) {
      return this.readdirSync(p, cred2);
    }
    async chmod(p, mode, cred2) {
      return this.chmodSync(p, mode, cred2);
    }
    async chown(p, new_uid, new_gid, cred2) {
      return this.chownSync(p, new_uid, new_gid, cred2);
    }
    async utimes(p, atime, mtime, cred2) {
      return this.utimesSync(p, atime, mtime, cred2);
    }
    async link(srcpath, dstpath, cred2) {
      return this.linkSync(srcpath, dstpath, cred2);
    }
    async symlink(srcpath, dstpath, type, cred2) {
      return this.symlinkSync(srcpath, dstpath, type, cred2);
    }
    async readlink(p, cred2) {
      return this.readlinkSync(p, cred2);
    }
  };
  __name(SynchronousFileSystem, "SynchronousFileSystem");

  // src/cred.ts
  var _Cred = class {
    constructor(uid, gid, suid, sgid, euid, egid) {
      this.uid = uid;
      this.gid = gid;
      this.suid = suid;
      this.sgid = sgid;
      this.euid = euid;
      this.egid = egid;
    }
  };
  var Cred = _Cred;
  __name(Cred, "Cred");
  Cred.Root = new _Cred(0, 0, 0, 0, 0, 0);

  // src/emulation/constants.ts
  var constants_exports = {};
  __export(constants_exports, {
    COPYFILE_EXCL: () => COPYFILE_EXCL,
    COPYFILE_FICLONE: () => COPYFILE_FICLONE,
    COPYFILE_FICLONE_FORCE: () => COPYFILE_FICLONE_FORCE,
    F_OK: () => F_OK,
    O_APPEND: () => O_APPEND,
    O_CREAT: () => O_CREAT,
    O_DIRECT: () => O_DIRECT,
    O_DIRECTORY: () => O_DIRECTORY,
    O_DSYNC: () => O_DSYNC,
    O_EXCL: () => O_EXCL,
    O_NOATIME: () => O_NOATIME,
    O_NOCTTY: () => O_NOCTTY,
    O_NOFOLLOW: () => O_NOFOLLOW,
    O_NONBLOCK: () => O_NONBLOCK,
    O_RDONLY: () => O_RDONLY,
    O_RDWR: () => O_RDWR,
    O_SYMLINK: () => O_SYMLINK,
    O_SYNC: () => O_SYNC,
    O_TRUNC: () => O_TRUNC,
    O_WRONLY: () => O_WRONLY,
    R_OK: () => R_OK,
    S_IFBLK: () => S_IFBLK,
    S_IFCHR: () => S_IFCHR,
    S_IFDIR: () => S_IFDIR,
    S_IFIFO: () => S_IFIFO,
    S_IFLNK: () => S_IFLNK,
    S_IFMT: () => S_IFMT,
    S_IFREG: () => S_IFREG,
    S_IFSOCK: () => S_IFSOCK,
    S_IRGRP: () => S_IRGRP,
    S_IROTH: () => S_IROTH,
    S_IRUSR: () => S_IRUSR,
    S_IRWXG: () => S_IRWXG,
    S_IRWXO: () => S_IRWXO,
    S_IRWXU: () => S_IRWXU,
    S_IWGRP: () => S_IWGRP,
    S_IWOTH: () => S_IWOTH,
    S_IWUSR: () => S_IWUSR,
    S_IXGRP: () => S_IXGRP,
    S_IXOTH: () => S_IXOTH,
    S_IXUSR: () => S_IXUSR,
    W_OK: () => W_OK,
    X_OK: () => X_OK
  });
  var F_OK = 0;
  var R_OK = 4;
  var W_OK = 2;
  var X_OK = 1;
  var COPYFILE_EXCL = 1;
  var COPYFILE_FICLONE = 2;
  var COPYFILE_FICLONE_FORCE = 4;
  var O_RDONLY = 0;
  var O_WRONLY = 1;
  var O_RDWR = 2;
  var O_CREAT = 64;
  var O_EXCL = 128;
  var O_NOCTTY = 256;
  var O_TRUNC = 512;
  var O_APPEND = 1024;
  var O_DIRECTORY = 65536;
  var O_NOATIME = 262144;
  var O_NOFOLLOW = 131072;
  var O_SYNC = 1052672;
  var O_DSYNC = 4096;
  var O_SYMLINK = 32768;
  var O_DIRECT = 16384;
  var O_NONBLOCK = 2048;
  var S_IFMT = 61440;
  var S_IFREG = 32768;
  var S_IFDIR = 16384;
  var S_IFCHR = 8192;
  var S_IFBLK = 24576;
  var S_IFIFO = 4096;
  var S_IFLNK = 40960;
  var S_IFSOCK = 49152;
  var S_IRWXU = 448;
  var S_IRUSR = 256;
  var S_IWUSR = 128;
  var S_IXUSR = 64;
  var S_IRWXG = 56;
  var S_IRGRP = 32;
  var S_IWGRP = 16;
  var S_IXGRP = 8;
  var S_IRWXO = 7;
  var S_IROTH = 4;
  var S_IWOTH = 2;
  var S_IXOTH = 1;

  // src/stats.ts
  var FileType = /* @__PURE__ */ ((FileType2) => {
    FileType2[FileType2["FILE"] = S_IFREG] = "FILE";
    FileType2[FileType2["DIRECTORY"] = S_IFDIR] = "DIRECTORY";
    FileType2[FileType2["SYMLINK"] = S_IFLNK] = "SYMLINK";
    return FileType2;
  })(FileType || {});
  var Stats = class {
    /**
     * Provides information about a particular entry in the file system.
     * @param itemType Type of the item (FILE, DIRECTORY, SYMLINK, or SOCKET)
     * @param size Size of the item in bytes. For directories/symlinks,
     *   this is normally the size of the struct that represents the item.
     * @param mode Unix-style file mode (e.g. 0o644)
     * @param atimeMs time of last access, in milliseconds since epoch
     * @param mtimeMs time of last modification, in milliseconds since epoch
     * @param ctimeMs time of last time file status was changed, in milliseconds since epoch
     * @param uid the id of the user that owns the file
     * @param gid the id of the group that owns the file
     * @param birthtimeMs time of file creation, in milliseconds since epoch
     */
    constructor(itemType, size, mode, atimeMs, mtimeMs, ctimeMs, uid, gid, birthtimeMs) {
      // ID of device containing file
      this.dev = 0;
      // inode number
      this.ino = 0;
      // device ID (if special file)
      this.rdev = 0;
      // number of hard links
      this.nlink = 1;
      // blocksize for file system I/O
      this.blksize = 4096;
      // user ID of owner
      this.uid = 0;
      // group ID of owner
      this.gid = 0;
      // Some file systems stash data on stats objects.
      this.fileData = null;
      this.size = size;
      let currentTime = 0;
      if (typeof atimeMs !== "number") {
        currentTime = Date.now();
        atimeMs = currentTime;
      }
      if (typeof mtimeMs !== "number") {
        if (!currentTime) {
          currentTime = Date.now();
        }
        mtimeMs = currentTime;
      }
      if (typeof ctimeMs !== "number") {
        if (!currentTime) {
          currentTime = Date.now();
        }
        ctimeMs = currentTime;
      }
      if (typeof birthtimeMs !== "number") {
        if (!currentTime) {
          currentTime = Date.now();
        }
        birthtimeMs = currentTime;
      }
      if (typeof uid !== "number") {
        uid = 0;
      }
      if (typeof gid !== "number") {
        gid = 0;
      }
      this.atimeMs = atimeMs;
      this.ctimeMs = ctimeMs;
      this.mtimeMs = mtimeMs;
      this.birthtimeMs = birthtimeMs;
      if (!mode) {
        switch (itemType) {
          case FileType.FILE:
            this.mode = 420;
            break;
          case FileType.DIRECTORY:
          default:
            this.mode = 511;
        }
      } else {
        this.mode = mode;
      }
      this.blocks = Math.ceil(size / 512);
      if ((this.mode & S_IFMT) == 0) {
        this.mode |= itemType;
      }
    }
    static fromBuffer(buffer) {
      const size = buffer.readUInt32LE(0), mode = buffer.readUInt32LE(4), atime = buffer.readDoubleLE(8), mtime = buffer.readDoubleLE(16), ctime = buffer.readDoubleLE(24), uid = buffer.readUInt32LE(32), gid = buffer.readUInt32LE(36);
      return new Stats(mode & S_IFMT, size, mode & ~S_IFMT, atime, mtime, ctime, uid, gid);
    }
    /**
     * Clones the stats object.
     */
    static clone(s) {
      return new Stats(s.mode & S_IFMT, s.size, s.mode & ~S_IFMT, s.atimeMs, s.mtimeMs, s.ctimeMs, s.uid, s.gid, s.birthtimeMs);
    }
    get atime() {
      return new Date(this.atimeMs);
    }
    get mtime() {
      return new Date(this.mtimeMs);
    }
    get ctime() {
      return new Date(this.ctimeMs);
    }
    get birthtime() {
      return new Date(this.birthtimeMs);
    }
    toBuffer() {
      const buffer = Buffer2.alloc(32);
      buffer.writeUInt32LE(this.size, 0);
      buffer.writeUInt32LE(this.mode, 4);
      buffer.writeDoubleLE(this.atime.getTime(), 8);
      buffer.writeDoubleLE(this.mtime.getTime(), 16);
      buffer.writeDoubleLE(this.ctime.getTime(), 24);
      buffer.writeUInt32LE(this.uid, 32);
      buffer.writeUInt32LE(this.gid, 36);
      return buffer;
    }
    /**
     * @return [Boolean] True if this item is a file.
     */
    isFile() {
      return (this.mode & S_IFMT) === S_IFREG;
    }
    /**
     * @return [Boolean] True if this item is a directory.
     */
    isDirectory() {
      return (this.mode & S_IFMT) === S_IFDIR;
    }
    /**
     * @return [Boolean] True if this item is a symbolic link (only valid through lstat)
     */
    isSymbolicLink() {
      return (this.mode & S_IFMT) === S_IFLNK;
    }
    /**
     * Checks if a given user/group has access to this item
     * @param mode The request access as 4 bits (unused, read, write, execute)
     * @param uid The requesting UID
     * @param gid The requesting GID
     * @returns [Boolean] True if the request has access, false if the request does not
     */
    hasAccess(mode, cred2) {
      if (cred2.euid === 0 || cred2.egid === 0) {
        return true;
      }
      const perms = this.mode & ~S_IFMT;
      let uMode = 15, gMode = 15, wMode = 15;
      if (cred2.euid == this.uid) {
        const uPerms = (3840 & perms) >> 8;
        uMode = (mode ^ uPerms) & mode;
      }
      if (cred2.egid == this.gid) {
        const gPerms = (240 & perms) >> 4;
        gMode = (mode ^ gPerms) & mode;
      }
      const wPerms = 15 & perms;
      wMode = (mode ^ wPerms) & mode;
      const result = uMode & gMode & wMode;
      return !result;
    }
    /**
     * Convert the current stats object into a cred object
     */
    getCred(uid = this.uid, gid = this.gid) {
      return new Cred(uid, gid, this.uid, this.gid, uid, gid);
    }
    /**
     * Change the mode of the file. We use this helper function to prevent messing
     * up the type of the file, which is encoded in mode.
     */
    chmod(mode) {
      this.mode = this.mode & S_IFMT | mode;
    }
    /**
     * Change the owner user/group of the file.
     * This function makes sure it is a valid UID/GID (that is, a 32 unsigned int)
     */
    chown(uid, gid) {
      if (!isNaN(+uid) && 0 <= +uid && +uid < 2 ** 32) {
        this.uid = uid;
      }
      if (!isNaN(+gid) && 0 <= +gid && +gid < 2 ** 32) {
        this.gid = gid;
      }
    }
    // We don't support the following types of files.
    isSocket() {
      return false;
    }
    isBlockDevice() {
      return false;
    }
    isCharacterDevice() {
      return false;
    }
    isFIFO() {
      return false;
    }
  };
  __name(Stats, "Stats");

  // src/inode.ts
  var Inode = class {
    constructor(id, size, mode, atime, mtime, ctime, uid, gid) {
      this.id = id;
      this.size = size;
      this.mode = mode;
      this.atime = atime;
      this.mtime = mtime;
      this.ctime = ctime;
      this.uid = uid;
      this.gid = gid;
    }
    /**
     * Converts the buffer into an Inode.
     */
    static fromBuffer(buffer) {
      if (buffer === void 0) {
        throw new Error("NO");
      }
      return new Inode(
        buffer.toString("ascii", 38),
        buffer.readUInt32LE(0),
        buffer.readUInt16LE(4),
        buffer.readDoubleLE(6),
        buffer.readDoubleLE(14),
        buffer.readDoubleLE(22),
        buffer.readUInt32LE(30),
        buffer.readUInt32LE(34)
      );
    }
    /**
     * Handy function that converts the Inode to a Node Stats object.
     */
    toStats() {
      return new Stats(
        (this.mode & 61440) === FileType.DIRECTORY ? FileType.DIRECTORY : FileType.FILE,
        this.size,
        this.mode,
        this.atime,
        this.mtime,
        this.ctime,
        this.uid,
        this.gid
      );
    }
    /**
     * Get the size of this Inode, in bytes.
     */
    getSize() {
      return 38 + this.id.length;
    }
    /**
     * Writes the inode into the start of the buffer.
     */
    toBuffer(buff = Buffer2.alloc(this.getSize())) {
      buff.writeUInt32LE(this.size, 0);
      buff.writeUInt16LE(this.mode, 4);
      buff.writeDoubleLE(this.atime, 6);
      buff.writeDoubleLE(this.mtime, 14);
      buff.writeDoubleLE(this.ctime, 22);
      buff.writeUInt32LE(this.uid, 30);
      buff.writeUInt32LE(this.gid, 34);
      buff.write(this.id, 38, this.id.length, "ascii");
      return buff;
    }
    /**
     * Updates the Inode using information from the stats object. Used by file
     * systems at sync time, e.g.:
     * - Program opens file and gets a File object.
     * - Program mutates file. File object is responsible for maintaining
     *   metadata changes locally -- typically in a Stats object.
     * - Program closes file. File object's metadata changes are synced with the
     *   file system.
     * @return True if any changes have occurred.
     */
    update(stats) {
      let hasChanged = false;
      if (this.size !== stats.size) {
        this.size = stats.size;
        hasChanged = true;
      }
      if (this.mode !== stats.mode) {
        this.mode = stats.mode;
        hasChanged = true;
      }
      const atimeMs = stats.atime.getTime();
      if (this.atime !== atimeMs) {
        this.atime = atimeMs;
        hasChanged = true;
      }
      const mtimeMs = stats.mtime.getTime();
      if (this.mtime !== mtimeMs) {
        this.mtime = mtimeMs;
        hasChanged = true;
      }
      const ctimeMs = stats.ctime.getTime();
      if (this.ctime !== ctimeMs) {
        this.ctime = ctimeMs;
        hasChanged = true;
      }
      if (this.uid !== stats.uid) {
        this.uid = stats.uid;
        hasChanged = true;
      }
      if (this.uid !== stats.uid) {
        this.uid = stats.uid;
        hasChanged = true;
      }
      return hasChanged;
    }
    // XXX: Copied from Stats. Should reconcile these two into something more
    //      compact.
    /**
     * @return [Boolean] True if this item is a file.
     */
    isFile() {
      return (this.mode & 61440) === FileType.FILE;
    }
    /**
     * @return [Boolean] True if this item is a directory.
     */
    isDirectory() {
      return (this.mode & 61440) === FileType.DIRECTORY;
    }
  };
  __name(Inode, "Inode");

  // src/generic/preload_file.ts
  var PreloadFile = class extends BaseFile {
    /**
     * Creates a file with the given path and, optionally, the given contents. Note
     * that, if contents is specified, it will be mutated by the file!
     * @param _fs The file system that created the file.
     * @param _path
     * @param _mode The mode that the file was opened using.
     *   Dictates permissions and where the file pointer starts.
     * @param _stat The stats object for the given file.
     *   PreloadFile will mutate this object. Note that this object must contain
     *   the appropriate mode that the file was opened as.
     * @param contents A buffer containing the entire
     *   contents of the file. PreloadFile will mutate this buffer. If not
     *   specified, we assume it is a new file.
     */
    constructor(_fs, _path, _flag, _stat, contents) {
      super();
      this._pos = 0;
      this._dirty = false;
      this._fs = _fs;
      this._path = _path;
      this._flag = _flag;
      this._stat = _stat;
      this._buffer = contents ? contents : Buffer2.alloc(0);
      if (this._stat.size !== this._buffer.length && this._flag.isReadable()) {
        throw new Error(`Invalid buffer: Buffer is ${this._buffer.length} long, yet Stats object specifies that file is ${this._stat.size} long.`);
      }
    }
    /**
     * NONSTANDARD: Get the underlying buffer for this file. !!DO NOT MUTATE!! Will mess up dirty tracking.
     */
    getBuffer() {
      return this._buffer;
    }
    /**
     * NONSTANDARD: Get underlying stats for this file. !!DO NOT MUTATE!!
     */
    getStats() {
      return this._stat;
    }
    getFlag() {
      return this._flag;
    }
    /**
     * Get the path to this file.
     * @return [String] The path to the file.
     */
    getPath() {
      return this._path;
    }
    /**
     * Get the current file position.
     *
     * We emulate the following bug mentioned in the Node documentation:
     * > On Linux, positional writes don't work when the file is opened in append
     *   mode. The kernel ignores the position argument and always appends the data
     *   to the end of the file.
     * @return [Number] The current file position.
     */
    getPos() {
      if (this._flag.isAppendable()) {
        return this._stat.size;
      }
      return this._pos;
    }
    /**
     * Advance the current file position by the indicated number of positions.
     * @param [Number] delta
     */
    advancePos(delta) {
      return this._pos += delta;
    }
    /**
     * Set the file position.
     * @param [Number] newPos
     */
    setPos(newPos) {
      return this._pos = newPos;
    }
    /**
     * **Core**: Asynchronous sync. Must be implemented by subclasses of this
     * class.
     * @param [Function(BrowserFS.ApiError)] cb
     */
    async sync() {
      this.syncSync();
    }
    /**
     * **Core**: Synchronous sync.
     */
    syncSync() {
      throw new ApiError(95 /* ENOTSUP */);
    }
    /**
     * **Core**: Asynchronous close. Must be implemented by subclasses of this
     * class.
     * @param [Function(BrowserFS.ApiError)] cb
     */
    async close() {
      this.closeSync();
    }
    /**
     * **Core**: Synchronous close.
     */
    closeSync() {
      throw new ApiError(95 /* ENOTSUP */);
    }
    /**
     * Asynchronous `stat`.
     * @param [Function(BrowserFS.ApiError, BrowserFS.node.fs.Stats)] cb
     */
    async stat() {
      return Stats.clone(this._stat);
    }
    /**
     * Synchronous `stat`.
     */
    statSync() {
      return Stats.clone(this._stat);
    }
    /**
     * Asynchronous truncate.
     * @param [Number] len
     * @param [Function(BrowserFS.ApiError)] cb
     */
    truncate(len) {
      this.truncateSync(len);
      if (this._flag.isSynchronous() && !getMount("/").metadata.synchronous) {
        return this.sync();
      }
    }
    /**
     * Synchronous truncate.
     * @param [Number] len
     */
    truncateSync(len) {
      this._dirty = true;
      if (!this._flag.isWriteable()) {
        throw new ApiError(1 /* EPERM */, "File not opened with a writeable mode.");
      }
      this._stat.mtimeMs = Date.now();
      if (len > this._buffer.length) {
        const buf = Buffer2.alloc(len - this._buffer.length, 0);
        this.writeSync(buf, 0, buf.length, this._buffer.length);
        if (this._flag.isSynchronous() && getMount("/").metadata.synchronous) {
          this.syncSync();
        }
        return;
      }
      this._stat.size = len;
      const newBuff = Buffer2.alloc(len);
      this._buffer.copy(newBuff, 0, 0, len);
      this._buffer = newBuff;
      if (this._flag.isSynchronous() && getMount("/").metadata.synchronous) {
        this.syncSync();
      }
    }
    /**
     * Write buffer to the file.
     * Note that it is unsafe to use fs.write multiple times on the same file
     * without waiting for the callback.
     * @param [BrowserFS.node.Buffer] buffer Buffer containing the data to write to
     *  the file.
     * @param [Number] offset Offset in the buffer to start reading data from.
     * @param [Number] length The amount of bytes to write to the file.
     * @param [Number] position Offset from the beginning of the file where this
     *   data should be written. If position is null, the data will be written at
     *   the current position.
     * @param [Function(BrowserFS.ApiError, Number, BrowserFS.node.Buffer)]
     *   cb The number specifies the number of bytes written into the file.
     */
    async write(buffer, offset, length, position) {
      return this.writeSync(buffer, offset, length, position);
    }
    /**
     * Write buffer to the file.
     * Note that it is unsafe to use fs.writeSync multiple times on the same file
     * without waiting for the callback.
     * @param [BrowserFS.node.Buffer] buffer Buffer containing the data to write to
     *  the file.
     * @param [Number] offset Offset in the buffer to start reading data from.
     * @param [Number] length The amount of bytes to write to the file.
     * @param [Number] position Offset from the beginning of the file where this
     *   data should be written. If position is null, the data will be written at
     *   the current position.
     * @return [Number]
     */
    writeSync(buffer, offset, length, position) {
      this._dirty = true;
      if (position === void 0 || position === null) {
        position = this.getPos();
      }
      if (!this._flag.isWriteable()) {
        throw new ApiError(1 /* EPERM */, "File not opened with a writeable mode.");
      }
      const endFp = position + length;
      if (endFp > this._stat.size) {
        this._stat.size = endFp;
        if (endFp > this._buffer.length) {
          const newBuff = Buffer2.alloc(endFp);
          this._buffer.copy(newBuff);
          this._buffer = newBuff;
        }
      }
      const len = buffer.copy(this._buffer, position, offset, offset + length);
      this._stat.mtimeMs = Date.now();
      if (this._flag.isSynchronous()) {
        this.syncSync();
        return len;
      }
      this.setPos(position + len);
      return len;
    }
    /**
     * Read data from the file.
     * @param [BrowserFS.node.Buffer] buffer The buffer that the data will be
     *   written to.
     * @param [Number] offset The offset within the buffer where writing will
     *   start.
     * @param [Number] length An integer specifying the number of bytes to read.
     * @param [Number] position An integer specifying where to begin reading from
     *   in the file. If position is null, data will be read from the current file
     *   position.
     * @param [Function(BrowserFS.ApiError, Number, BrowserFS.node.Buffer)] cb The
     *   number is the number of bytes read
     */
    async read(buffer, offset, length, position) {
      return { bytesRead: this.readSync(buffer, offset, length, position), buffer };
    }
    /**
     * Read data from the file.
     * @param [BrowserFS.node.Buffer] buffer The buffer that the data will be
     *   written to.
     * @param [Number] offset The offset within the buffer where writing will
     *   start.
     * @param [Number] length An integer specifying the number of bytes to read.
     * @param [Number] position An integer specifying where to begin reading from
     *   in the file. If position is null, data will be read from the current file
     *   position.
     * @return [Number]
     */
    readSync(buffer, offset, length, position) {
      if (!this._flag.isReadable()) {
        throw new ApiError(1 /* EPERM */, "File not opened with a readable mode.");
      }
      if (position === void 0 || position === null) {
        position = this.getPos();
      }
      const endRead = position + length;
      if (endRead > this._stat.size) {
        length = this._stat.size - position;
      }
      const rv = this._buffer.copy(buffer, offset, position, position + length);
      this._stat.atimeMs = Date.now();
      this._pos = position + length;
      return rv;
    }
    /**
     * Asynchronous `fchmod`.
     * @param [Number|String] mode
     */
    async chmod(mode) {
      this.chmodSync(mode);
    }
    /**
     * Synchronous `fchmod`.
     * @param [Number] mode
     */
    chmodSync(mode) {
      if (!this._fs.metadata.supportsProperties) {
        throw new ApiError(95 /* ENOTSUP */);
      }
      this._dirty = true;
      this._stat.chmod(mode);
      this.syncSync();
    }
    /**
     * Asynchronous `fchown`.
     * @param [Number] uid
     * @param [Number] gid
     */
    async chown(uid, gid) {
      this.chownSync(uid, gid);
    }
    /**
     * Synchronous `fchown`.
     * @param [Number] uid
     * @param [Number] gid
     */
    chownSync(uid, gid) {
      if (!this._fs.metadata.supportsProperties) {
        throw new ApiError(95 /* ENOTSUP */);
      }
      this._dirty = true;
      this._stat.chown(uid, gid);
      this.syncSync();
    }
    isDirty() {
      return this._dirty;
    }
    /**
     * Resets the dirty bit. Should only be called after a sync has completed successfully.
     */
    resetDirty() {
      this._dirty = false;
    }
  };
  __name(PreloadFile, "PreloadFile");

  // src/generic/key_value_filesystem.ts
  var ROOT_NODE_ID = "/";
  var emptyDirNode = null;
  function getEmptyDirNode() {
    if (emptyDirNode) {
      return emptyDirNode;
    }
    return emptyDirNode = Buffer2.from("{}");
  }
  __name(getEmptyDirNode, "getEmptyDirNode");
  function GenerateRandomID() {
    return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, function(c) {
      const r = Math.random() * 16 | 0;
      const v = c === "x" ? r : r & 3 | 8;
      return v.toString(16);
    });
  }
  __name(GenerateRandomID, "GenerateRandomID");
  var LRUNode = class {
    constructor(key, value) {
      this.key = key;
      this.value = value;
      this.prev = null;
      this.next = null;
    }
  };
  __name(LRUNode, "LRUNode");
  var LRUCache = class {
    constructor(limit) {
      this.limit = limit;
      this.size = 0;
      this.map = {};
      this.head = null;
      this.tail = null;
    }
    /**
     * Change or add a new value in the cache
     * We overwrite the entry if it already exists
     */
    set(key, value) {
      const node = new LRUNode(key, value);
      if (this.map[key]) {
        this.map[key].value = node.value;
        this.remove(node.key);
      } else {
        if (this.size >= this.limit) {
          delete this.map[this.tail.key];
          this.size--;
          this.tail = this.tail.prev;
          this.tail.next = null;
        }
      }
      this.setHead(node);
    }
    /* Retrieve a single entry from the cache */
    get(key) {
      if (this.map[key]) {
        const value = this.map[key].value;
        const node = new LRUNode(key, value);
        this.remove(key);
        this.setHead(node);
        return value;
      } else {
        return null;
      }
    }
    /* Remove a single entry from the cache */
    remove(key) {
      const node = this.map[key];
      if (!node) {
        return;
      }
      if (node.prev !== null) {
        node.prev.next = node.next;
      } else {
        this.head = node.next;
      }
      if (node.next !== null) {
        node.next.prev = node.prev;
      } else {
        this.tail = node.prev;
      }
      delete this.map[key];
      this.size--;
    }
    /* Resets the entire cache - Argument limit is optional to be reset */
    removeAll() {
      this.size = 0;
      this.map = {};
      this.head = null;
      this.tail = null;
    }
    setHead(node) {
      node.next = this.head;
      node.prev = null;
      if (this.head !== null) {
        this.head.prev = node;
      }
      this.head = node;
      if (this.tail === null) {
        this.tail = node;
      }
      this.size++;
      this.map[node.key] = node;
    }
  };
  __name(LRUCache, "LRUCache");
  var SimpleSyncRWTransaction = class {
    constructor(store) {
      this.store = store;
      /**
       * Stores data in the keys we modify prior to modifying them.
       * Allows us to roll back commits.
       */
      this.originalData = {};
      /**
       * List of keys modified in this transaction, if any.
       */
      this.modifiedKeys = [];
    }
    get(key) {
      const val = this.store.get(key);
      this.stashOldValue(key, val);
      return val;
    }
    put(key, data, overwrite) {
      this.markModified(key);
      return this.store.put(key, data, overwrite);
    }
    del(key) {
      this.markModified(key);
      this.store.del(key);
    }
    commit() {
    }
    abort() {
      for (const key of this.modifiedKeys) {
        const value = this.originalData[key];
        if (!value) {
          this.store.del(key);
        } else {
          this.store.put(key, value, true);
        }
      }
    }
    _has(key) {
      return Object.prototype.hasOwnProperty.call(this.originalData, key);
    }
    /**
     * Stashes given key value pair into `originalData` if it doesn't already
     * exist. Allows us to stash values the program is requesting anyway to
     * prevent needless `get` requests if the program modifies the data later
     * on during the transaction.
     */
    stashOldValue(key, value) {
      if (!this._has(key)) {
        this.originalData[key] = value;
      }
    }
    /**
     * Marks the given key as modified, and stashes its value if it has not been
     * stashed already.
     */
    markModified(key) {
      if (this.modifiedKeys.indexOf(key) === -1) {
        this.modifiedKeys.push(key);
        if (!this._has(key)) {
          this.originalData[key] = this.store.get(key);
        }
      }
    }
  };
  __name(SimpleSyncRWTransaction, "SimpleSyncRWTransaction");
  var SyncKeyValueFile = class extends PreloadFile {
    constructor(_fs, _path, _flag, _stat, contents) {
      super(_fs, _path, _flag, _stat, contents);
    }
    syncSync() {
      if (this.isDirty()) {
        this._fs._syncSync(this.getPath(), this.getBuffer(), this.getStats());
        this.resetDirty();
      }
    }
    closeSync() {
      this.syncSync();
    }
  };
  __name(SyncKeyValueFile, "SyncKeyValueFile");
  var SyncKeyValueFileSystem = class extends SynchronousFileSystem {
    constructor(options) {
      super();
      this.store = options.store;
      this.makeRootDirectory();
    }
    static isAvailable() {
      return true;
    }
    getName() {
      return this.store.name();
    }
    isReadOnly() {
      return false;
    }
    supportsSymlinks() {
      return false;
    }
    supportsProps() {
      return true;
    }
    supportsSynch() {
      return true;
    }
    /**
     * Delete all contents stored in the file system.
     */
    empty() {
      this.store.clear();
      this.makeRootDirectory();
    }
    accessSync(p, mode, cred2) {
      const tx = this.store.beginTransaction("readonly"), node = this.findINode(tx, p);
      if (!node.toStats().hasAccess(mode, cred2)) {
        throw ApiError.EACCES(p);
      }
    }
    renameSync(oldPath, newPath, cred2) {
      const tx = this.store.beginTransaction("readwrite"), oldParent = dirname(oldPath), oldName = basename(oldPath), newParent = dirname(newPath), newName = basename(newPath), oldDirNode = this.findINode(tx, oldParent), oldDirList = this.getDirListing(tx, oldParent, oldDirNode);
      if (!oldDirNode.toStats().hasAccess(W_OK, cred2)) {
        throw ApiError.EACCES(oldPath);
      }
      if (!oldDirList[oldName]) {
        throw ApiError.ENOENT(oldPath);
      }
      const nodeId = oldDirList[oldName];
      delete oldDirList[oldName];
      if ((newParent + "/").indexOf(oldPath + "/") === 0) {
        throw new ApiError(16 /* EBUSY */, oldParent);
      }
      let newDirNode, newDirList;
      if (newParent === oldParent) {
        newDirNode = oldDirNode;
        newDirList = oldDirList;
      } else {
        newDirNode = this.findINode(tx, newParent);
        newDirList = this.getDirListing(tx, newParent, newDirNode);
      }
      if (newDirList[newName]) {
        const newNameNode = this.getINode(tx, newPath, newDirList[newName]);
        if (newNameNode.isFile()) {
          try {
            tx.del(newNameNode.id);
            tx.del(newDirList[newName]);
          } catch (e) {
            tx.abort();
            throw e;
          }
        } else {
          throw ApiError.EPERM(newPath);
        }
      }
      newDirList[newName] = nodeId;
      try {
        tx.put(oldDirNode.id, Buffer2.from(JSON.stringify(oldDirList)), true);
        tx.put(newDirNode.id, Buffer2.from(JSON.stringify(newDirList)), true);
      } catch (e) {
        tx.abort();
        throw e;
      }
      tx.commit();
    }
    statSync(p, cred2) {
      const stats = this.findINode(this.store.beginTransaction("readonly"), p).toStats();
      if (!stats.hasAccess(R_OK, cred2)) {
        throw ApiError.EACCES(p);
      }
      return stats;
    }
    createFileSync(p, flag, mode, cred2) {
      const tx = this.store.beginTransaction("readwrite"), data = Buffer2.alloc(0), newFile = this.commitNewFile(tx, p, FileType.FILE, mode, cred2, data);
      return new SyncKeyValueFile(this, p, flag, newFile.toStats(), data);
    }
    openFileSync(p, flag, cred2) {
      const tx = this.store.beginTransaction("readonly"), node = this.findINode(tx, p), data = tx.get(node.id);
      if (!node.toStats().hasAccess(flag.getMode(), cred2)) {
        throw ApiError.EACCES(p);
      }
      if (data === void 0) {
        throw ApiError.ENOENT(p);
      }
      return new SyncKeyValueFile(this, p, flag, node.toStats(), data);
    }
    unlinkSync(p, cred2) {
      this.removeEntry(p, false, cred2);
    }
    rmdirSync(p, cred2) {
      if (this.readdirSync(p, cred2).length > 0) {
        throw ApiError.ENOTEMPTY(p);
      } else {
        this.removeEntry(p, true, cred2);
      }
    }
    mkdirSync(p, mode, cred2) {
      const tx = this.store.beginTransaction("readwrite"), data = Buffer2.from("{}");
      this.commitNewFile(tx, p, FileType.DIRECTORY, mode, cred2, data);
    }
    readdirSync(p, cred2) {
      const tx = this.store.beginTransaction("readonly");
      const node = this.findINode(tx, p);
      if (!node.toStats().hasAccess(R_OK, cred2)) {
        throw ApiError.EACCES(p);
      }
      return Object.keys(this.getDirListing(tx, p, node));
    }
    chmodSync(p, mode, cred2) {
      const fd = this.openFileSync(p, FileFlag.getFileFlag("r+"), cred2);
      fd.chmodSync(mode);
    }
    chownSync(p, new_uid, new_gid, cred2) {
      const fd = this.openFileSync(p, FileFlag.getFileFlag("r+"), cred2);
      fd.chownSync(new_uid, new_gid);
    }
    _syncSync(p, data, stats) {
      const tx = this.store.beginTransaction("readwrite"), fileInodeId = this._findINode(tx, dirname(p), basename(p)), fileInode = this.getINode(tx, p, fileInodeId), inodeChanged = fileInode.update(stats);
      try {
        tx.put(fileInode.id, data, true);
        if (inodeChanged) {
          tx.put(fileInodeId, fileInode.toBuffer(), true);
        }
      } catch (e) {
        tx.abort();
        throw e;
      }
      tx.commit();
    }
    /**
     * Checks if the root directory exists. Creates it if it doesn't.
     */
    makeRootDirectory() {
      const tx = this.store.beginTransaction("readwrite");
      if (tx.get(ROOT_NODE_ID) === void 0) {
        const currTime = (/* @__PURE__ */ new Date()).getTime(), dirInode = new Inode(GenerateRandomID(), 4096, 511 | FileType.DIRECTORY, currTime, currTime, currTime, 0, 0);
        tx.put(dirInode.id, getEmptyDirNode(), false);
        tx.put(ROOT_NODE_ID, dirInode.toBuffer(), false);
        tx.commit();
      }
    }
    /**
     * Helper function for findINode.
     * @param parent The parent directory of the file we are attempting to find.
     * @param filename The filename of the inode we are attempting to find, minus
     *   the parent.
     * @return string The ID of the file's inode in the file system.
     */
    _findINode(tx, parent, filename, visited = /* @__PURE__ */ new Set()) {
      const currentPath = posix.join(parent, filename);
      if (visited.has(currentPath)) {
        throw new ApiError(5 /* EIO */, "Infinite loop detected while finding inode", currentPath);
      }
      visited.add(currentPath);
      const readDirectory = /* @__PURE__ */ __name((inode) => {
        const dirList = this.getDirListing(tx, parent, inode);
        if (dirList[filename]) {
          return dirList[filename];
        } else {
          throw ApiError.ENOENT(resolve(parent, filename));
        }
      }, "readDirectory");
      if (parent === ".") {
        parent = cwd();
      }
      if (parent === "/") {
        if (filename === "") {
          return ROOT_NODE_ID;
        } else {
          return readDirectory(this.getINode(tx, parent, ROOT_NODE_ID));
        }
      } else {
        return readDirectory(this.getINode(tx, parent + sep + filename, this._findINode(tx, dirname(parent), basename(parent), visited)));
      }
    }
    /**
     * Finds the Inode of the given path.
     * @param p The path to look up.
     * @return The Inode of the path p.
     * @todo memoize/cache
     */
    findINode(tx, p) {
      return this.getINode(tx, p, this._findINode(tx, dirname(p), basename(p)));
    }
    /**
     * Given the ID of a node, retrieves the corresponding Inode.
     * @param tx The transaction to use.
     * @param p The corresponding path to the file (used for error messages).
     * @param id The ID to look up.
     */
    getINode(tx, p, id) {
      const inode = tx.get(id);
      if (inode === void 0) {
        throw ApiError.ENOENT(p);
      }
      return Inode.fromBuffer(inode);
    }
    /**
     * Given the Inode of a directory, retrieves the corresponding directory
     * listing.
     */
    getDirListing(tx, p, inode) {
      if (!inode.isDirectory()) {
        throw ApiError.ENOTDIR(p);
      }
      const data = tx.get(inode.id);
      if (data === void 0) {
        throw ApiError.ENOENT(p);
      }
      return JSON.parse(data.toString());
    }
    /**
     * Creates a new node under a random ID. Retries 5 times before giving up in
     * the exceedingly unlikely chance that we try to reuse a random GUID.
     * @return The GUID that the data was stored under.
     */
    addNewNode(tx, data) {
      const retries = 0;
      let currId;
      while (retries < 5) {
        try {
          currId = GenerateRandomID();
          tx.put(currId, data, false);
          return currId;
        } catch (e) {
        }
      }
      throw new ApiError(5 /* EIO */, "Unable to commit data to key-value store.");
    }
    /**
     * Commits a new file (well, a FILE or a DIRECTORY) to the file system with
     * the given mode.
     * Note: This will commit the transaction.
     * @param p The path to the new file.
     * @param type The type of the new file.
     * @param mode The mode to create the new file with.
     * @param data The data to store at the file's data node.
     * @return The Inode for the new file.
     */
    commitNewFile(tx, p, type, mode, cred2, data) {
      const parentDir = dirname(p), fname = basename(p), parentNode = this.findINode(tx, parentDir), dirListing = this.getDirListing(tx, parentDir, parentNode), currTime = (/* @__PURE__ */ new Date()).getTime();
      if (!parentNode.toStats().hasAccess(4, cred2)) {
        throw ApiError.EACCES(p);
      }
      if (p === "/") {
        throw ApiError.EEXIST(p);
      }
      if (dirListing[fname]) {
        throw ApiError.EEXIST(p);
      }
      let fileNode;
      try {
        const dataId = this.addNewNode(tx, data);
        fileNode = new Inode(dataId, data.length, mode | type, currTime, currTime, currTime, cred2.uid, cred2.gid);
        const fileNodeId = this.addNewNode(tx, fileNode.toBuffer());
        dirListing[fname] = fileNodeId;
        tx.put(parentNode.id, Buffer2.from(JSON.stringify(dirListing)), true);
      } catch (e) {
        tx.abort();
        throw e;
      }
      tx.commit();
      return fileNode;
    }
    /**
     * Remove all traces of the given path from the file system.
     * @param p The path to remove from the file system.
     * @param isDir Does the path belong to a directory, or a file?
     * @todo Update mtime.
     */
    removeEntry(p, isDir, cred2) {
      const tx = this.store.beginTransaction("readwrite"), parent = dirname(p), parentNode = this.findINode(tx, parent), parentListing = this.getDirListing(tx, parent, parentNode), fileName = basename(p);
      if (!parentListing[fileName]) {
        throw ApiError.ENOENT(p);
      }
      const fileNodeId = parentListing[fileName];
      const fileNode = this.getINode(tx, p, fileNodeId);
      if (!fileNode.toStats().hasAccess(W_OK, cred2)) {
        throw ApiError.EACCES(p);
      }
      delete parentListing[fileName];
      if (!isDir && fileNode.isDirectory()) {
        throw ApiError.EISDIR(p);
      } else if (isDir && !fileNode.isDirectory()) {
        throw ApiError.ENOTDIR(p);
      }
      try {
        tx.del(fileNode.id);
        tx.del(fileNodeId);
        tx.put(parentNode.id, Buffer2.from(JSON.stringify(parentListing)), true);
      } catch (e) {
        tx.abort();
        throw e;
      }
      tx.commit();
    }
  };
  __name(SyncKeyValueFileSystem, "SyncKeyValueFileSystem");
  var AsyncKeyValueFile = class extends PreloadFile {
    constructor(_fs, _path, _flag, _stat, contents) {
      super(_fs, _path, _flag, _stat, contents);
    }
    async sync() {
      if (!this.isDirty()) {
        return;
      }
      await this._fs._sync(this.getPath(), this.getBuffer(), this.getStats());
      this.resetDirty();
    }
    async close() {
      this.sync();
    }
  };
  __name(AsyncKeyValueFile, "AsyncKeyValueFile");
  var AsyncKeyValueFileSystem = class extends BaseFileSystem {
    constructor(cacheSize) {
      super();
      this._cache = null;
      if (cacheSize > 0) {
        this._cache = new LRUCache(cacheSize);
      }
    }
    static isAvailable() {
      return true;
    }
    /**
     * Initializes the file system. Typically called by subclasses' async
     * constructors.
     */
    async init(store) {
      this.store = store;
      await this.makeRootDirectory();
    }
    getName() {
      return this.store.name();
    }
    isReadOnly() {
      return false;
    }
    supportsSymlinks() {
      return false;
    }
    supportsProps() {
      return true;
    }
    supportsSynch() {
      return false;
    }
    /**
     * Delete all contents stored in the file system.
     */
    async empty() {
      if (this._cache) {
        this._cache.removeAll();
      }
      await this.store.clear();
      await this.makeRootDirectory();
    }
    async access(p, mode, cred2) {
      const tx = this.store.beginTransaction("readonly");
      const inode = await this.findINode(tx, p);
      if (!inode) {
        throw ApiError.ENOENT(p);
      }
      if (!inode.toStats().hasAccess(mode, cred2)) {
        throw ApiError.EACCES(p);
      }
    }
    /**
     * @todo Make rename compatible with the cache.
     */
    async rename(oldPath, newPath, cred2) {
      const c = this._cache;
      if (this._cache) {
        this._cache = null;
        c.removeAll();
      }
      try {
        const tx = this.store.beginTransaction("readwrite"), oldParent = dirname(oldPath), oldName = basename(oldPath), newParent = dirname(newPath), newName = basename(newPath), oldDirNode = await this.findINode(tx, oldParent), oldDirList = await this.getDirListing(tx, oldParent, oldDirNode);
        if (!oldDirNode.toStats().hasAccess(W_OK, cred2)) {
          throw ApiError.EACCES(oldPath);
        }
        if (!oldDirList[oldName]) {
          throw ApiError.ENOENT(oldPath);
        }
        const nodeId = oldDirList[oldName];
        delete oldDirList[oldName];
        if ((newParent + "/").indexOf(oldPath + "/") === 0) {
          throw new ApiError(16 /* EBUSY */, oldParent);
        }
        let newDirNode, newDirList;
        if (newParent === oldParent) {
          newDirNode = oldDirNode;
          newDirList = oldDirList;
        } else {
          newDirNode = await this.findINode(tx, newParent);
          newDirList = await this.getDirListing(tx, newParent, newDirNode);
        }
        if (newDirList[newName]) {
          const newNameNode = await this.getINode(tx, newPath, newDirList[newName]);
          if (newNameNode.isFile()) {
            try {
              await tx.del(newNameNode.id);
              await tx.del(newDirList[newName]);
            } catch (e) {
              await tx.abort();
              throw e;
            }
          } else {
            throw ApiError.EPERM(newPath);
          }
        }
        newDirList[newName] = nodeId;
        try {
          await tx.put(oldDirNode.id, Buffer2.from(JSON.stringify(oldDirList)), true);
          await tx.put(newDirNode.id, Buffer2.from(JSON.stringify(newDirList)), true);
        } catch (e) {
          await tx.abort();
          throw e;
        }
        await tx.commit();
      } finally {
        if (c) {
          this._cache = c;
        }
      }
    }
    async stat(p, cred2) {
      const tx = this.store.beginTransaction("readonly");
      const inode = await this.findINode(tx, p);
      const stats = inode.toStats();
      if (!stats.hasAccess(R_OK, cred2)) {
        throw ApiError.EACCES(p);
      }
      return stats;
    }
    async createFile(p, flag, mode, cred2) {
      const tx = this.store.beginTransaction("readwrite"), data = Buffer2.alloc(0), newFile = await this.commitNewFile(tx, p, FileType.FILE, mode, cred2, data);
      return new AsyncKeyValueFile(this, p, flag, newFile.toStats(), data);
    }
    async openFile(p, flag, cred2) {
      const tx = this.store.beginTransaction("readonly"), node = await this.findINode(tx, p), data = await tx.get(node.id);
      if (!node.toStats().hasAccess(flag.getMode(), cred2)) {
        throw ApiError.EACCES(p);
      }
      if (data === void 0) {
        throw ApiError.ENOENT(p);
      }
      return new AsyncKeyValueFile(this, p, flag, node.toStats(), data);
    }
    async unlink(p, cred2) {
      return this.removeEntry(p, false, cred2);
    }
    async rmdir(p, cred2) {
      const list = await this.readdir(p, cred2);
      if (list.length > 0) {
        throw ApiError.ENOTEMPTY(p);
      }
      await this.removeEntry(p, true, cred2);
    }
    async mkdir(p, mode, cred2) {
      const tx = this.store.beginTransaction("readwrite"), data = Buffer2.from("{}");
      await this.commitNewFile(tx, p, FileType.DIRECTORY, mode, cred2, data);
    }
    async readdir(p, cred2) {
      const tx = this.store.beginTransaction("readonly");
      const node = await this.findINode(tx, p);
      if (!node.toStats().hasAccess(R_OK, cred2)) {
        throw ApiError.EACCES(p);
      }
      return Object.keys(await this.getDirListing(tx, p, node));
    }
    async chmod(p, mode, cred2) {
      const fd = await this.openFile(p, FileFlag.getFileFlag("r+"), cred2);
      await fd.chmod(mode);
    }
    async chown(p, new_uid, new_gid, cred2) {
      const fd = await this.openFile(p, FileFlag.getFileFlag("r+"), cred2);
      await fd.chown(new_uid, new_gid);
    }
    async _sync(p, data, stats) {
      const tx = this.store.beginTransaction("readwrite"), fileInodeId = await this._findINode(tx, dirname(p), basename(p)), fileInode = await this.getINode(tx, p, fileInodeId), inodeChanged = fileInode.update(stats);
      try {
        await tx.put(fileInode.id, data, true);
        if (inodeChanged) {
          await tx.put(fileInodeId, fileInode.toBuffer(), true);
        }
      } catch (e) {
        await tx.abort();
        throw e;
      }
      await tx.commit();
    }
    /**
     * Checks if the root directory exists. Creates it if it doesn't.
     */
    async makeRootDirectory() {
      const tx = this.store.beginTransaction("readwrite");
      if (await tx.get(ROOT_NODE_ID) === void 0) {
        const currTime = (/* @__PURE__ */ new Date()).getTime(), dirInode = new Inode(GenerateRandomID(), 4096, 511 | FileType.DIRECTORY, currTime, currTime, currTime, 0, 0);
        await tx.put(dirInode.id, getEmptyDirNode(), false);
        await tx.put(ROOT_NODE_ID, dirInode.toBuffer(), false);
        await tx.commit();
      }
    }
    /**
     * Helper function for findINode.
     * @param parent The parent directory of the file we are attempting to find.
     * @param filename The filename of the inode we are attempting to find, minus
     *   the parent.
     */
    async _findINode(tx, parent, filename, visited = /* @__PURE__ */ new Set()) {
      const currentPath = posix.join(parent, filename);
      if (visited.has(currentPath)) {
        throw new ApiError(5 /* EIO */, "Infinite loop detected while finding inode", currentPath);
      }
      visited.add(currentPath);
      if (this._cache) {
        const id = this._cache.get(currentPath);
        if (id) {
          return id;
        }
      }
      if (parent === "/") {
        if (filename === "") {
          if (this._cache) {
            this._cache.set(currentPath, ROOT_NODE_ID);
          }
          return ROOT_NODE_ID;
        } else {
          const inode = await this.getINode(tx, parent, ROOT_NODE_ID);
          const dirList = await this.getDirListing(tx, parent, inode);
          if (dirList[filename]) {
            const id = dirList[filename];
            if (this._cache) {
              this._cache.set(currentPath, id);
            }
            return id;
          } else {
            throw ApiError.ENOENT(resolve(parent, filename));
          }
        }
      } else {
        const inode = await this.findINode(tx, parent, visited);
        const dirList = await this.getDirListing(tx, parent, inode);
        if (dirList[filename]) {
          const id = dirList[filename];
          if (this._cache) {
            this._cache.set(currentPath, id);
          }
          return id;
        } else {
          throw ApiError.ENOENT(resolve(parent, filename));
        }
      }
    }
    /**
     * Finds the Inode of the given path.
     * @param p The path to look up.
     * @todo memoize/cache
     */
    async findINode(tx, p, visited = /* @__PURE__ */ new Set()) {
      const id = await this._findINode(tx, dirname(p), basename(p), visited);
      return this.getINode(tx, p, id);
    }
    /**
     * Given the ID of a node, retrieves the corresponding Inode.
     * @param tx The transaction to use.
     * @param p The corresponding path to the file (used for error messages).
     * @param id The ID to look up.
     */
    async getINode(tx, p, id) {
      const data = await tx.get(id);
      if (!data) {
        throw ApiError.ENOENT(p);
      }
      return Inode.fromBuffer(data);
    }
    /**
     * Given the Inode of a directory, retrieves the corresponding directory
     * listing.
     */
    async getDirListing(tx, p, inode) {
      if (!inode.isDirectory()) {
        throw ApiError.ENOTDIR(p);
      }
      const data = await tx.get(inode.id);
      try {
        return JSON.parse(data.toString());
      } catch (e) {
        throw ApiError.ENOENT(p);
      }
    }
    /**
     * Adds a new node under a random ID. Retries 5 times before giving up in
     * the exceedingly unlikely chance that we try to reuse a random GUID.
     */
    async addNewNode(tx, data) {
      let retries = 0;
      const reroll = /* @__PURE__ */ __name(async () => {
        if (++retries === 5) {
          throw new ApiError(5 /* EIO */, "Unable to commit data to key-value store.");
        } else {
          const currId = GenerateRandomID();
          const committed = await tx.put(currId, data, false);
          if (!committed) {
            return reroll();
          } else {
            return currId;
          }
        }
      }, "reroll");
      return reroll();
    }
    /**
     * Commits a new file (well, a FILE or a DIRECTORY) to the file system with
     * the given mode.
     * Note: This will commit the transaction.
     * @param p The path to the new file.
     * @param type The type of the new file.
     * @param mode The mode to create the new file with.
     * @param cred The UID/GID to create the file with
     * @param data The data to store at the file's data node.
     */
    async commitNewFile(tx, p, type, mode, cred2, data) {
      const parentDir = dirname(p), fname = basename(p), parentNode = await this.findINode(tx, parentDir), dirListing = await this.getDirListing(tx, parentDir, parentNode), currTime = (/* @__PURE__ */ new Date()).getTime();
      if (!parentNode.toStats().hasAccess(W_OK, cred2)) {
        throw ApiError.EACCES(p);
      }
      if (p === "/") {
        throw ApiError.EEXIST(p);
      }
      if (dirListing[fname]) {
        await tx.abort();
        throw ApiError.EEXIST(p);
      }
      try {
        const dataId = await this.addNewNode(tx, data);
        const fileNode = new Inode(dataId, data.length, mode | type, currTime, currTime, currTime, cred2.uid, cred2.gid);
        const fileNodeId = await this.addNewNode(tx, fileNode.toBuffer());
        dirListing[fname] = fileNodeId;
        await tx.put(parentNode.id, Buffer2.from(JSON.stringify(dirListing)), true);
        await tx.commit();
        return fileNode;
      } catch (e) {
        tx.abort();
        throw e;
      }
    }
    /**
     * Remove all traces of the given path from the file system.
     * @param p The path to remove from the file system.
     * @param isDir Does the path belong to a directory, or a file?
     * @todo Update mtime.
     */
    /**
     * Remove all traces of the given path from the file system.
     * @param p The path to remove from the file system.
     * @param isDir Does the path belong to a directory, or a file?
     * @todo Update mtime.
     */
    async removeEntry(p, isDir, cred2) {
      if (this._cache) {
        this._cache.remove(p);
      }
      const tx = this.store.beginTransaction("readwrite"), parent = dirname(p), parentNode = await this.findINode(tx, parent), parentListing = await this.getDirListing(tx, parent, parentNode), fileName = basename(p);
      if (!parentListing[fileName]) {
        throw ApiError.ENOENT(p);
      }
      const fileNodeId = parentListing[fileName];
      const fileNode = await this.getINode(tx, p, fileNodeId);
      if (!fileNode.toStats().hasAccess(W_OK, cred2)) {
        throw ApiError.EACCES(p);
      }
      delete parentListing[fileName];
      if (!isDir && fileNode.isDirectory()) {
        throw ApiError.EISDIR(p);
      } else if (isDir && !fileNode.isDirectory()) {
        throw ApiError.ENOTDIR(p);
      }
      try {
        await tx.del(fileNode.id);
        await tx.del(fileNodeId);
        await tx.put(parentNode.id, Buffer2.from(JSON.stringify(parentListing)), true);
      } catch (e) {
        await tx.abort();
        throw e;
      }
      await tx.commit();
    }
  };
  __name(AsyncKeyValueFileSystem, "AsyncKeyValueFileSystem");

  // src/utils.ts
  function _min(d0, d1, d2, bx, ay) {
    return Math.min(d0 + 1, d1 + 1, d2 + 1, bx === ay ? d1 : d1 + 1);
  }
  __name(_min, "_min");
  function levenshtein(a, b) {
    if (a === b) {
      return 0;
    }
    if (a.length > b.length) {
      [a, b] = [b, a];
    }
    let la = a.length;
    let lb = b.length;
    while (la > 0 && a.charCodeAt(la - 1) === b.charCodeAt(lb - 1)) {
      la--;
      lb--;
    }
    let offset = 0;
    while (offset < la && a.charCodeAt(offset) === b.charCodeAt(offset)) {
      offset++;
    }
    la -= offset;
    lb -= offset;
    if (la === 0 || lb === 1) {
      return lb;
    }
    const vector = new Array(la << 1);
    for (let y = 0; y < la; ) {
      vector[la + y] = a.charCodeAt(offset + y);
      vector[y] = ++y;
    }
    let x;
    let d0;
    let d1;
    let d2;
    let d3;
    for (x = 0; x + 3 < lb; ) {
      const bx0 = b.charCodeAt(offset + (d0 = x));
      const bx1 = b.charCodeAt(offset + (d1 = x + 1));
      const bx2 = b.charCodeAt(offset + (d2 = x + 2));
      const bx3 = b.charCodeAt(offset + (d3 = x + 3));
      let dd2 = x += 4;
      for (let y = 0; y < la; ) {
        const ay = vector[la + y];
        const dy = vector[y];
        d0 = _min(dy, d0, d1, bx0, ay);
        d1 = _min(d0, d1, d2, bx1, ay);
        d2 = _min(d1, d2, d3, bx2, ay);
        dd2 = _min(d2, d3, dd2, bx3, ay);
        vector[y++] = dd2;
        d3 = d2;
        d2 = d1;
        d1 = d0;
        d0 = dy;
      }
    }
    let dd = 0;
    for (; x < lb; ) {
      const bx0 = b.charCodeAt(offset + (d0 = x));
      dd = ++x;
      for (let y = 0; y < la; y++) {
        const dy = vector[y];
        vector[y] = dd = dy < d0 || dd < d0 ? dy > dd ? dd + 1 : dy + 1 : bx0 === vector[la + y] ? d0 : d0 + 1;
        d0 = dy;
      }
    }
    return dd;
  }
  __name(levenshtein, "levenshtein");
  async function checkOptions(backend, opts) {
    const optsInfo = backend.Options;
    const fsName = backend.Name;
    let pendingValidators = 0;
    let callbackCalled = false;
    let loopEnded = false;
    for (const optName in optsInfo) {
      if (Object.prototype.hasOwnProperty.call(optsInfo, optName)) {
        const opt = optsInfo[optName];
        const providedValue = opts && opts[optName];
        if (providedValue === void 0 || providedValue === null) {
          if (!opt.optional) {
            const incorrectOptions = Object.keys(opts).filter((o) => !(o in optsInfo)).map((a) => {
              return { str: a, distance: levenshtein(optName, a) };
            }).filter((o) => o.distance < 5).sort((a, b) => a.distance - b.distance);
            if (callbackCalled) {
              return;
            }
            callbackCalled = true;
            throw new ApiError(
              22 /* EINVAL */,
              `[${fsName}] Required option '${optName}' not provided.${incorrectOptions.length > 0 ? ` You provided unrecognized option '${incorrectOptions[0].str}'; perhaps you meant to type '${optName}'.` : ""}
Option description: ${opt.description}`
            );
          }
        } else {
          let typeMatches = false;
          if (Array.isArray(opt.type)) {
            typeMatches = opt.type.indexOf(typeof providedValue) !== -1;
          } else {
            typeMatches = typeof providedValue === opt.type;
          }
          if (!typeMatches) {
            if (callbackCalled) {
              return;
            }
            callbackCalled = true;
            throw new ApiError(
              22 /* EINVAL */,
              `[${fsName}] Value provided for option ${optName} is not the proper type. Expected ${Array.isArray(opt.type) ? `one of {${opt.type.join(", ")}}` : opt.type}, but received ${typeof providedValue}
Option description: ${opt.description}`
            );
          } else if (opt.validator) {
            pendingValidators++;
            try {
              await opt.validator(providedValue);
            } catch (e) {
              if (!callbackCalled) {
                if (e) {
                  callbackCalled = true;
                  throw e;
                }
                pendingValidators--;
                if (pendingValidators === 0 && loopEnded) {
                  return;
                }
              }
            }
          }
        }
      }
    }
    loopEnded = true;
    if (pendingValidators === 0 && !callbackCalled) {
      return;
    }
  }
  __name(checkOptions, "checkOptions");
  var setImmediate = typeof globalThis.setImmediate == "function" ? globalThis.setImmediate : (cb) => setTimeout(cb, 0);

  // src/backends/backend.ts
  function CreateBackend(options, cb) {
    cb = typeof options === "function" ? options : cb;
    checkOptions(this, options);
    const fs2 = new this(typeof options === "function" ? {} : options);
    if (typeof cb != "function") {
      return fs2.whenReady();
    }
    fs2.whenReady().then((fs3) => cb(null, fs3)).catch((err) => cb(err));
  }
  __name(CreateBackend, "CreateBackend");

  // src/backends/InMemory.ts
  var InMemoryStore = class {
    constructor() {
      this.store = /* @__PURE__ */ new Map();
    }
    name() {
      return InMemoryFileSystem.Name;
    }
    clear() {
      this.store.clear();
    }
    beginTransaction(type) {
      return new SimpleSyncRWTransaction(this);
    }
    get(key) {
      return this.store.get(key);
    }
    put(key, data, overwrite) {
      if (!overwrite && this.store.has(key)) {
        return false;
      }
      this.store.set(key, data);
      return true;
    }
    del(key) {
      this.store.delete(key);
    }
  };
  __name(InMemoryStore, "InMemoryStore");
  var _InMemoryFileSystem = class extends SyncKeyValueFileSystem {
    constructor() {
      super({ store: new InMemoryStore() });
    }
  };
  var InMemoryFileSystem = _InMemoryFileSystem;
  __name(InMemoryFileSystem, "InMemoryFileSystem");
  InMemoryFileSystem.Name = "InMemory";
  InMemoryFileSystem.Create = CreateBackend.bind(_InMemoryFileSystem);
  InMemoryFileSystem.Options = {};

  // src/emulation/shared.ts
  function _toUnixTimestamp(time) {
    if (typeof time === "number") {
      return time;
    } else if (time instanceof Date) {
      return time.getTime() / 1e3;
    }
    throw new Error("Cannot parse time: " + time);
  }
  __name(_toUnixTimestamp, "_toUnixTimestamp");
  function normalizeMode(mode, def) {
    switch (typeof mode) {
      case "number":
        return mode;
      case "string":
        const trueMode = parseInt(mode, 8);
        if (!isNaN(trueMode)) {
          return trueMode;
        }
        return def;
      default:
        return def;
    }
  }
  __name(normalizeMode, "normalizeMode");
  function normalizeTime(time) {
    if (time instanceof Date) {
      return time;
    }
    if (typeof time === "number") {
      return new Date(time * 1e3);
    }
    throw new ApiError(22 /* EINVAL */, `Invalid time.`);
  }
  __name(normalizeTime, "normalizeTime");
  function normalizePath(p) {
    if (p.indexOf("\0") >= 0) {
      throw new ApiError(22 /* EINVAL */, "Path must be a string without null bytes.");
    }
    if (p === "") {
      throw new ApiError(22 /* EINVAL */, "Path must not be empty.");
    }
    p = p.replaceAll(/\/+/g, "/");
    return posix.resolve(p);
  }
  __name(normalizePath, "normalizePath");
  function normalizeOptions(options, defEnc, defFlag, defMode) {
    switch (options === null ? "null" : typeof options) {
      case "object":
        return {
          encoding: typeof options["encoding"] !== "undefined" ? options["encoding"] : defEnc,
          flag: typeof options["flag"] !== "undefined" ? options["flag"] : defFlag,
          mode: normalizeMode(options["mode"], defMode)
        };
      case "string":
        return {
          encoding: options,
          flag: defFlag,
          mode: defMode
        };
      case "null":
      case "undefined":
      case "function":
        return {
          encoding: defEnc,
          flag: defFlag,
          mode: defMode
        };
      default:
        throw new TypeError(`"options" must be a string or an object, got ${typeof options} instead.`);
    }
  }
  __name(normalizeOptions, "normalizeOptions");
  function nop() {
  }
  __name(nop, "nop");
  var cred;
  function setCred(val) {
    cred = val;
  }
  __name(setCred, "setCred");
  var fdMap = /* @__PURE__ */ new Map();
  var nextFd = 100;
  function getFdForFile(file) {
    const fd = nextFd++;
    fdMap.set(fd, file);
    return fd;
  }
  __name(getFdForFile, "getFdForFile");
  function fd2file(fd) {
    if (!fdMap.has(fd)) {
      throw new ApiError(9 /* EBADF */, "Invalid file descriptor.");
    }
    return fdMap.get(fd);
  }
  __name(fd2file, "fd2file");
  var mounts = /* @__PURE__ */ new Map();
  InMemoryFileSystem.Create().then((fs2) => mount("/", fs2));
  function getMount(mountPoint) {
    return mounts.get(mountPoint);
  }
  __name(getMount, "getMount");
  function getMounts() {
    return Object.fromEntries(mounts.entries());
  }
  __name(getMounts, "getMounts");
  function mount(mountPoint, fs2) {
    if (mountPoint[0] !== "/") {
      mountPoint = "/" + mountPoint;
    }
    mountPoint = posix.resolve(mountPoint);
    if (mounts.has(mountPoint)) {
      throw new ApiError(22 /* EINVAL */, "Mount point " + mountPoint + " is already in use.");
    }
    mounts.set(mountPoint, fs2);
  }
  __name(mount, "mount");
  function umount(mountPoint) {
    if (mountPoint[0] !== "/") {
      mountPoint = `/${mountPoint}`;
    }
    mountPoint = posix.resolve(mountPoint);
    if (!mounts.has(mountPoint)) {
      throw new ApiError(22 /* EINVAL */, "Mount point " + mountPoint + " is already unmounted.");
    }
    mounts.delete(mountPoint);
  }
  __name(umount, "umount");
  function resolveFS(path) {
    const sortedMounts = [...mounts].sort((a, b) => a[0].length > b[0].length ? -1 : 1);
    for (const [mountPoint, fs2] of sortedMounts) {
      if (mountPoint.length <= path.length && path.startsWith(mountPoint)) {
        path = path.slice(mountPoint.length > 1 ? mountPoint.length : 0);
        if (path === "") {
          path = "/";
        }
        return { fs: fs2, path, mountPoint };
      }
    }
    throw new ApiError(5 /* EIO */, "BrowserFS not initialized with a file system");
  }
  __name(resolveFS, "resolveFS");
  function fixPaths(text, paths) {
    for (const [from, to] of Object.entries(paths)) {
      text = text.replaceAll(from, to);
    }
    return text;
  }
  __name(fixPaths, "fixPaths");
  function fixError(e, paths) {
    e.stack = fixPaths(e.stack, paths);
    e.message = fixPaths(e.message, paths);
    return e;
  }
  __name(fixError, "fixError");
  function initialize(mountMapping) {
    if (mountMapping["/"]) {
      umount("/");
    }
    for (const [point, fs2] of Object.entries(mountMapping)) {
      const FS = fs2.constructor;
      if (!FS.isAvailable()) {
        throw new ApiError(22 /* EINVAL */, `Can not mount "${point}" since the filesystem is unavailable.`);
      }
      mount(point, fs2);
    }
  }
  __name(initialize, "initialize");

  // src/emulation/promises.ts
  var promises_exports = {};
  __export(promises_exports, {
    access: () => access,
    appendFile: () => appendFile,
    chmod: () => chmod,
    chown: () => chown,
    close: () => close,
    constants: () => constants_exports,
    createReadStream: () => createReadStream,
    createWriteStream: () => createWriteStream,
    exists: () => exists,
    fchmod: () => fchmod,
    fchown: () => fchown,
    fdatasync: () => fdatasync,
    fstat: () => fstat,
    fsync: () => fsync,
    ftruncate: () => ftruncate,
    futimes: () => futimes,
    lchmod: () => lchmod,
    lchown: () => lchown,
    link: () => link,
    lstat: () => lstat,
    lutimes: () => lutimes,
    mkdir: () => mkdir,
    open: () => open,
    read: () => read,
    readFile: () => readFile,
    readdir: () => readdir,
    readlink: () => readlink,
    realpath: () => realpath,
    rename: () => rename,
    rmdir: () => rmdir,
    stat: () => stat,
    symlink: () => symlink,
    truncate: () => truncate,
    unlink: () => unlink,
    unwatchFile: () => unwatchFile,
    utimes: () => utimes,
    watch: () => watch,
    watchFile: () => watchFile,
    write: () => write,
    writeFile: () => writeFile
  });
  async function doOp(...[name, resolveSymlinks, path, ...args]) {
    path = normalizePath(path);
    const { fs: fs2, path: resolvedPath } = resolveFS(resolveSymlinks && await exists(path) ? await realpath(path) : path);
    try {
      return fs2[name](resolvedPath, ...args);
    } catch (e) {
      throw fixError(e, { [resolvedPath]: path });
    }
  }
  __name(doOp, "doOp");
  async function rename(oldPath, newPath) {
    oldPath = normalizePath(oldPath);
    newPath = normalizePath(newPath);
    const _old = resolveFS(oldPath);
    const _new = resolveFS(newPath);
    const paths = { [_old.path]: oldPath, [_new.path]: newPath };
    try {
      if (_old === _new) {
        return _old.fs.rename(_old.path, _new.path, cred);
      }
      const data = await readFile(oldPath);
      await writeFile(newPath, data);
      await unlink(oldPath);
    } catch (e) {
      throw fixError(e, paths);
    }
  }
  __name(rename, "rename");
  async function exists(path) {
    path = normalizePath(path);
    try {
      const { fs: fs2, path: resolvedPath } = resolveFS(path);
      return fs2.exists(resolvedPath, cred);
    } catch (e) {
      if (e.errno == 2 /* ENOENT */) {
        return false;
      }
      throw e;
    }
  }
  __name(exists, "exists");
  async function stat(path) {
    return doOp("stat", true, path, cred);
  }
  __name(stat, "stat");
  async function lstat(path) {
    return doOp("stat", false, path, cred);
  }
  __name(lstat, "lstat");
  async function truncate(path, len = 0) {
    if (len < 0) {
      throw new ApiError(22 /* EINVAL */);
    }
    return doOp("truncate", true, path, len, cred);
  }
  __name(truncate, "truncate");
  async function unlink(path) {
    return doOp("unlink", false, path, cred);
  }
  __name(unlink, "unlink");
  async function open(path, flag, mode = 420) {
    const file = await doOp("open", true, path, FileFlag.getFileFlag(flag), normalizeMode(mode, 420), cred);
    return getFdForFile(file);
  }
  __name(open, "open");
  async function readFile(filename, arg2 = {}) {
    const options = normalizeOptions(arg2, null, "r", null);
    const flag = FileFlag.getFileFlag(options.flag);
    if (!flag.isReadable()) {
      throw new ApiError(22 /* EINVAL */, "Flag passed to readFile must allow for reading.");
    }
    return doOp("readFile", true, filename, options.encoding, flag, cred);
  }
  __name(readFile, "readFile");
  async function writeFile(filename, data, arg3) {
    const options = normalizeOptions(arg3, "utf8", "w", 420);
    const flag = FileFlag.getFileFlag(options.flag);
    if (!flag.isWriteable()) {
      throw new ApiError(22 /* EINVAL */, "Flag passed to writeFile must allow for writing.");
    }
    return doOp("writeFile", true, filename, data, options.encoding, flag, options.mode, cred);
  }
  __name(writeFile, "writeFile");
  async function appendFile(filename, data, arg3) {
    const options = normalizeOptions(arg3, "utf8", "a", 420);
    const flag = FileFlag.getFileFlag(options.flag);
    if (!flag.isAppendable()) {
      throw new ApiError(22 /* EINVAL */, "Flag passed to appendFile must allow for appending.");
    }
    return doOp("appendFile", true, filename, data, options.encoding, flag, options.mode, cred);
  }
  __name(appendFile, "appendFile");
  async function fstat(fd) {
    return fd2file(fd).stat();
  }
  __name(fstat, "fstat");
  async function close(fd) {
    await fd2file(fd).close();
    fdMap.delete(fd);
    return;
  }
  __name(close, "close");
  async function ftruncate(fd, len = 0) {
    const file = fd2file(fd);
    if (len < 0) {
      throw new ApiError(22 /* EINVAL */);
    }
    return file.truncate(len);
  }
  __name(ftruncate, "ftruncate");
  async function fsync(fd) {
    return fd2file(fd).sync();
  }
  __name(fsync, "fsync");
  async function fdatasync(fd) {
    return fd2file(fd).datasync();
  }
  __name(fdatasync, "fdatasync");
  async function write(fd, arg2, arg3, arg4, arg5) {
    let buffer, offset = 0, length, position;
    if (typeof arg2 === "string") {
      position = typeof arg3 === "number" ? arg3 : null;
      const encoding = typeof arg4 === "string" ? arg4 : "utf8";
      offset = 0;
      buffer = Buffer2.from(arg2, encoding);
      length = buffer.length;
    } else {
      buffer = arg2;
      offset = arg3;
      length = arg4;
      position = typeof arg5 === "number" ? arg5 : null;
    }
    const file = fd2file(fd);
    if (position === void 0 || position === null) {
      position = file.getPos();
    }
    return file.write(buffer, offset, length, position);
  }
  __name(write, "write");
  async function read(fd, buffer, offset, length, position) {
    const file = fd2file(fd);
    if (isNaN(+position)) {
      position = file.getPos();
    }
    return file.read(buffer, offset, length, position);
  }
  __name(read, "read");
  async function fchown(fd, uid, gid) {
    return fd2file(fd).chown(uid, gid);
  }
  __name(fchown, "fchown");
  async function fchmod(fd, mode) {
    const numMode = typeof mode === "string" ? parseInt(mode, 8) : mode;
    return fd2file(fd).chmod(numMode);
  }
  __name(fchmod, "fchmod");
  async function futimes(fd, atime, mtime) {
    return fd2file(fd).utimes(normalizeTime(atime), normalizeTime(mtime));
  }
  __name(futimes, "futimes");
  async function rmdir(path) {
    return doOp("rmdir", true, path, cred);
  }
  __name(rmdir, "rmdir");
  async function mkdir(path, mode) {
    return doOp("mkdir", true, path, normalizeMode(mode, 511), cred);
  }
  __name(mkdir, "mkdir");
  async function readdir(path) {
    path = normalizePath(path);
    const entries = await doOp("readdir", true, path, cred);
    const points = [...mounts.keys()];
    for (const point of points) {
      if (point.startsWith(path)) {
        const entry = point.slice(path.length);
        if (entry.includes("/") || entry.length == 0) {
          continue;
        }
        entries.push(entry);
      }
    }
    return entries;
  }
  __name(readdir, "readdir");
  async function link(srcpath, dstpath) {
    dstpath = normalizePath(dstpath);
    return doOp("link", false, srcpath, dstpath, cred);
  }
  __name(link, "link");
  async function symlink(srcpath, dstpath, type = "file") {
    if (!["file", "dir", "junction"].includes(type)) {
      throw new ApiError(22 /* EINVAL */, "Invalid type: " + type);
    }
    dstpath = normalizePath(dstpath);
    return doOp("symlink", false, srcpath, dstpath, type, cred);
  }
  __name(symlink, "symlink");
  async function readlink(path) {
    return doOp("readlink", false, path, cred);
  }
  __name(readlink, "readlink");
  async function chown(path, uid, gid) {
    return doOp("chown", true, path, uid, gid, cred);
  }
  __name(chown, "chown");
  async function lchown(path, uid, gid) {
    return doOp("chown", false, path, uid, gid, cred);
  }
  __name(lchown, "lchown");
  async function chmod(path, mode) {
    const numMode = normalizeMode(mode, -1);
    if (numMode < 0) {
      throw new ApiError(22 /* EINVAL */, `Invalid mode.`);
    }
    return doOp("chmod", true, path, numMode, cred);
  }
  __name(chmod, "chmod");
  async function lchmod(path, mode) {
    const numMode = normalizeMode(mode, -1);
    if (numMode < 1) {
      throw new ApiError(22 /* EINVAL */, `Invalid mode.`);
    }
    return doOp("chmod", false, normalizePath(path), numMode, cred);
  }
  __name(lchmod, "lchmod");
  async function utimes(path, atime, mtime) {
    return doOp("utimes", true, path, normalizeTime(atime), normalizeTime(mtime), cred);
  }
  __name(utimes, "utimes");
  async function lutimes(path, atime, mtime) {
    return doOp("utimes", false, path, normalizeTime(atime), normalizeTime(mtime), cred);
  }
  __name(lutimes, "lutimes");
  async function realpath(path, cache = {}) {
    path = normalizePath(path);
    const { fs: fs2, path: resolvedPath, mountPoint } = resolveFS(path);
    try {
      const stats = await fs2.stat(resolvedPath, cred);
      if (!stats.isSymbolicLink()) {
        return path;
      }
      const dst = mountPoint + normalizePath(await fs2.readlink(resolvedPath, cred));
      return realpath(dst);
    } catch (e) {
      throw fixError(e, { [resolvedPath]: path });
    }
  }
  __name(realpath, "realpath");
  async function watchFile(filename, arg2, listener = nop) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(watchFile, "watchFile");
  async function unwatchFile(filename, listener = nop) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(unwatchFile, "unwatchFile");
  async function watch(filename, arg2, listener = nop) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(watch, "watch");
  async function access(path, mode = 384) {
    return doOp("access", true, path, mode, cred);
  }
  __name(access, "access");
  async function createReadStream(path, options) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(createReadStream, "createReadStream");
  async function createWriteStream(path, options) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(createWriteStream, "createWriteStream");

  // src/emulation/callbacks.ts
  function rename2(oldPath, newPath, cb = nop) {
    rename(oldPath, newPath).then(() => cb()).catch(cb);
  }
  __name(rename2, "rename");
  function exists2(path, cb = nop) {
    exists(path).then(cb).catch(() => cb(false));
  }
  __name(exists2, "exists");
  function stat2(path, cb = nop) {
    stat(path).then((stats) => cb(null, stats)).catch(cb);
  }
  __name(stat2, "stat");
  function lstat2(path, cb = nop) {
    lstat(path).then((stats) => cb(null, stats)).catch(cb);
  }
  __name(lstat2, "lstat");
  function truncate2(path, arg2 = 0, cb = nop) {
    cb = typeof arg2 === "function" ? arg2 : cb;
    const len = typeof arg2 === "number" ? arg2 : 0;
    truncate(path, len).then(() => cb()).catch(cb);
  }
  __name(truncate2, "truncate");
  function unlink2(path, cb = nop) {
    unlink(path).then(() => cb()).catch(cb);
  }
  __name(unlink2, "unlink");
  function open2(path, flag, arg2, cb = nop) {
    const mode = normalizeMode(arg2, 420);
    cb = typeof arg2 === "function" ? arg2 : cb;
    open(path, flag, mode).then((fd) => cb(null, fd)).catch(cb);
  }
  __name(open2, "open");
  function readFile2(filename, arg2 = {}, cb = nop) {
    cb = typeof arg2 === "function" ? arg2 : cb;
    readFile(filename, typeof arg2 === "function" ? null : arg2);
  }
  __name(readFile2, "readFile");
  function writeFile2(filename, data, arg3 = {}, cb = nop) {
    cb = typeof arg3 === "function" ? arg3 : cb;
    writeFile(filename, data, typeof arg3 === "function" ? void 0 : arg3);
  }
  __name(writeFile2, "writeFile");
  function appendFile2(filename, data, arg3, cb = nop) {
    cb = typeof arg3 === "function" ? arg3 : cb;
    appendFile(filename, data, typeof arg3 === "function" ? null : arg3);
  }
  __name(appendFile2, "appendFile");
  function fstat2(fd, cb = nop) {
    fstat(fd).then((stats) => cb(null, stats)).catch(cb);
  }
  __name(fstat2, "fstat");
  function close2(fd, cb = nop) {
    close(fd).then(() => cb()).catch(cb);
  }
  __name(close2, "close");
  function ftruncate2(fd, arg2, cb = nop) {
    const length = typeof arg2 === "number" ? arg2 : 0;
    cb = typeof arg2 === "function" ? arg2 : cb;
    ftruncate(fd, length);
  }
  __name(ftruncate2, "ftruncate");
  function fsync2(fd, cb = nop) {
    fsync(fd).then(() => cb()).catch(cb);
  }
  __name(fsync2, "fsync");
  function fdatasync2(fd, cb = nop) {
    fdatasync(fd).then(() => cb()).catch(cb);
  }
  __name(fdatasync2, "fdatasync");
  function write2(fd, arg2, arg3, arg4, arg5, cb = nop) {
    let buffer, offset, length, position = null, encoding;
    if (typeof arg2 === "string") {
      encoding = "utf8";
      switch (typeof arg3) {
        case "function":
          cb = arg3;
          break;
        case "number":
          position = arg3;
          encoding = typeof arg4 === "string" ? arg4 : "utf8";
          cb = typeof arg5 === "function" ? arg5 : cb;
          break;
        default:
          cb = typeof arg4 === "function" ? arg4 : typeof arg5 === "function" ? arg5 : cb;
          cb(new ApiError(22 /* EINVAL */, "Invalid arguments."));
          return;
      }
      buffer = Buffer2.from(arg2, encoding);
      offset = 0;
      length = buffer.length;
      const _cb = cb;
      write(fd, buffer, offset, length, position).then((bytesWritten) => _cb(null, bytesWritten, buffer.toString(encoding))).catch(_cb);
    } else {
      buffer = arg2;
      offset = arg3;
      length = arg4;
      position = typeof arg5 === "number" ? arg5 : null;
      const _cb = typeof arg5 === "function" ? arg5 : cb;
      write(fd, buffer, offset, length, position).then((bytesWritten) => _cb(null, bytesWritten, buffer)).catch(_cb);
    }
  }
  __name(write2, "write");
  function read2(fd, buffer, offset, length, position, cb = nop) {
    read(fd, buffer, offset, length, position).then(({ bytesRead, buffer: buffer2 }) => cb(null, bytesRead, buffer2)).catch(cb);
  }
  __name(read2, "read");
  function fchown2(fd, uid, gid, cb = nop) {
    fchown(fd, uid, gid).then(() => cb()).catch(cb);
  }
  __name(fchown2, "fchown");
  function fchmod2(fd, mode, cb) {
    fchmod(fd, mode).then(() => cb()).catch(cb);
  }
  __name(fchmod2, "fchmod");
  function futimes2(fd, atime, mtime, cb = nop) {
    futimes(fd, atime, mtime).then(() => cb()).catch(cb);
  }
  __name(futimes2, "futimes");
  function rmdir2(path, cb = nop) {
    rmdir(path).then(() => cb()).catch(cb);
  }
  __name(rmdir2, "rmdir");
  function mkdir2(path, mode, cb = nop) {
    mkdir(path, mode).then(() => cb()).catch(cb);
  }
  __name(mkdir2, "mkdir");
  function readdir2(path, cb = nop) {
    readdir(path).then((entries) => cb(null, entries)).catch(cb);
  }
  __name(readdir2, "readdir");
  function link2(srcpath, dstpath, cb = nop) {
    link(srcpath, dstpath).then(() => cb()).catch(cb);
  }
  __name(link2, "link");
  function symlink2(srcpath, dstpath, arg3, cb = nop) {
    const type = typeof arg3 === "string" ? arg3 : "file";
    cb = typeof arg3 === "function" ? arg3 : cb;
    symlink(srcpath, dstpath, typeof arg3 === "function" ? null : arg3).then(() => cb()).catch(cb);
  }
  __name(symlink2, "symlink");
  function readlink2(path, cb = nop) {
    readlink(path).then((result) => cb(null, result)).catch(cb);
  }
  __name(readlink2, "readlink");
  function chown2(path, uid, gid, cb = nop) {
    chown(path, uid, gid).then(() => cb()).catch(cb);
  }
  __name(chown2, "chown");
  function lchown2(path, uid, gid, cb = nop) {
    lchown(path, uid, gid).then(() => cb()).catch(cb);
  }
  __name(lchown2, "lchown");
  function chmod2(path, mode, cb = nop) {
    chmod(path, mode).then(() => cb()).catch(cb);
  }
  __name(chmod2, "chmod");
  function lchmod2(path, mode, cb = nop) {
    lchmod(path, mode).then(() => cb()).catch(cb);
  }
  __name(lchmod2, "lchmod");
  function utimes2(path, atime, mtime, cb = nop) {
    utimes(path, atime, mtime).then(() => cb()).catch(cb);
  }
  __name(utimes2, "utimes");
  function lutimes2(path, atime, mtime, cb = nop) {
    lutimes(path, atime, mtime).then(() => cb()).catch(cb);
  }
  __name(lutimes2, "lutimes");
  function realpath2(path, arg2, cb = nop) {
    const cache = typeof arg2 === "object" ? arg2 : {};
    cb = typeof arg2 === "function" ? arg2 : cb;
    realpath(path, typeof arg2 === "function" ? null : arg2).then((result) => cb(null, result)).catch(cb);
  }
  __name(realpath2, "realpath");
  function access2(path, arg2, cb = nop) {
    const mode = typeof arg2 === "number" ? arg2 : R_OK;
    cb = typeof arg2 === "function" ? arg2 : cb;
    access(path, typeof arg2 === "function" ? null : arg2).then(() => cb()).catch(cb);
  }
  __name(access2, "access");
  function watchFile2(filename, arg2, listener = nop) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(watchFile2, "watchFile");
  function unwatchFile2(filename, listener = nop) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(unwatchFile2, "unwatchFile");
  function watch2(filename, arg2, listener = nop) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(watch2, "watch");
  function createReadStream2(path, options) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(createReadStream2, "createReadStream");
  function createWriteStream2(path, options) {
    throw new ApiError(95 /* ENOTSUP */);
  }
  __name(createWriteStream2, "createWriteStream");

  // src/emulation/sync.ts
  function doOp2(...[name, resolveSymlinks, path, ...args]) {
    path = normalizePath(path);
    const { fs: fs2, path: resolvedPath } = resolveFS(resolveSymlinks && existsSync(path) ? realpathSync(path) : path);
    try {
      return fs2[name](resolvedPath, ...args);
    } catch (e) {
      throw fixError(e, { [resolvedPath]: path });
    }
  }
  __name(doOp2, "doOp");
  function renameSync(oldPath, newPath) {
    oldPath = normalizePath(oldPath);
    newPath = normalizePath(newPath);
    const _old = resolveFS(oldPath);
    const _new = resolveFS(newPath);
    const paths = { [_old.path]: oldPath, [_new.path]: newPath };
    try {
      if (_old === _new) {
        return _old.fs.renameSync(_old.path, _new.path, cred);
      }
      const data = readFileSync(oldPath);
      writeFileSync(newPath, data);
      unlinkSync(oldPath);
    } catch (e) {
      throw fixError(e, paths);
    }
  }
  __name(renameSync, "renameSync");
  function existsSync(path) {
    path = normalizePath(path);
    try {
      const { fs: fs2, path: resolvedPath } = resolveFS(path);
      return fs2.existsSync(resolvedPath, cred);
    } catch (e) {
      if (e.errno == 2 /* ENOENT */) {
        return false;
      }
      throw e;
    }
  }
  __name(existsSync, "existsSync");
  function statSync(path) {
    return doOp2("statSync", true, path, cred);
  }
  __name(statSync, "statSync");
  function lstatSync(path) {
    return doOp2("statSync", false, path, cred);
  }
  __name(lstatSync, "lstatSync");
  function truncateSync(path, len = 0) {
    if (len < 0) {
      throw new ApiError(22 /* EINVAL */);
    }
    return doOp2("truncateSync", true, path, len, cred);
  }
  __name(truncateSync, "truncateSync");
  function unlinkSync(path) {
    return doOp2("unlinkSync", false, path, cred);
  }
  __name(unlinkSync, "unlinkSync");
  function openSync(path, flag, mode = 420) {
    const file = doOp2("openSync", true, path, FileFlag.getFileFlag(flag), normalizeMode(mode, 420), cred);
    return getFdForFile(file);
  }
  __name(openSync, "openSync");
  function readFileSync(filename, arg2 = {}) {
    const options = normalizeOptions(arg2, null, "r", null);
    const flag = FileFlag.getFileFlag(options.flag);
    if (!flag.isReadable()) {
      throw new ApiError(22 /* EINVAL */, "Flag passed to readFile must allow for reading.");
    }
    return doOp2("readFileSync", true, filename, options.encoding, flag, cred);
  }
  __name(readFileSync, "readFileSync");
  function writeFileSync(filename, data, arg3) {
    const options = normalizeOptions(arg3, "utf8", "w", 420);
    const flag = FileFlag.getFileFlag(options.flag);
    if (!flag.isWriteable()) {
      throw new ApiError(22 /* EINVAL */, "Flag passed to writeFile must allow for writing.");
    }
    return doOp2("writeFileSync", true, filename, data, options.encoding, flag, options.mode, cred);
  }
  __name(writeFileSync, "writeFileSync");
  function appendFileSync(filename, data, arg3) {
    const options = normalizeOptions(arg3, "utf8", "a", 420);
    const flag = FileFlag.getFileFlag(options.flag);
    if (!flag.isAppendable()) {
      throw new ApiError(22 /* EINVAL */, "Flag passed to appendFile must allow for appending.");
    }
    return doOp2("appendFileSync", true, filename, data, options.encoding, flag, options.mode, cred);
  }
  __name(appendFileSync, "appendFileSync");
  function fstatSync(fd) {
    return fd2file(fd).statSync();
  }
  __name(fstatSync, "fstatSync");
  function closeSync(fd) {
    fd2file(fd).closeSync();
    fdMap.delete(fd);
  }
  __name(closeSync, "closeSync");
  function ftruncateSync(fd, len = 0) {
    const file = fd2file(fd);
    if (len < 0) {
      throw new ApiError(22 /* EINVAL */);
    }
    file.truncateSync(len);
  }
  __name(ftruncateSync, "ftruncateSync");
  function fsyncSync(fd) {
    fd2file(fd).syncSync();
  }
  __name(fsyncSync, "fsyncSync");
  function fdatasyncSync(fd) {
    fd2file(fd).datasyncSync();
  }
  __name(fdatasyncSync, "fdatasyncSync");
  function writeSync(fd, arg2, arg3, arg4, arg5) {
    let buffer, offset = 0, length, position;
    if (typeof arg2 === "string") {
      position = typeof arg3 === "number" ? arg3 : null;
      const encoding = typeof arg4 === "string" ? arg4 : "utf8";
      offset = 0;
      buffer = Buffer2.from(arg2, encoding);
      length = buffer.length;
    } else {
      buffer = arg2;
      offset = arg3;
      length = arg4;
      position = typeof arg5 === "number" ? arg5 : null;
    }
    const file = fd2file(fd);
    if (position === void 0 || position === null) {
      position = file.getPos();
    }
    return file.writeSync(buffer, offset, length, position);
  }
  __name(writeSync, "writeSync");
  function readSync(fd, buffer, opts, length, position) {
    const file = fd2file(fd);
    let offset = opts;
    if (typeof opts == "object") {
      ({ offset, length, position } = opts);
    }
    if (isNaN(+position)) {
      position = file.getPos();
    }
    return file.readSync(buffer, offset, length, position);
  }
  __name(readSync, "readSync");
  function fchownSync(fd, uid, gid) {
    fd2file(fd).chownSync(uid, gid);
  }
  __name(fchownSync, "fchownSync");
  function fchmodSync(fd, mode) {
    const numMode = typeof mode === "string" ? parseInt(mode, 8) : mode;
    fd2file(fd).chmodSync(numMode);
  }
  __name(fchmodSync, "fchmodSync");
  function futimesSync(fd, atime, mtime) {
    fd2file(fd).utimesSync(normalizeTime(atime), normalizeTime(mtime));
  }
  __name(futimesSync, "futimesSync");
  function rmdirSync(path) {
    return doOp2("rmdirSync", true, path, cred);
  }
  __name(rmdirSync, "rmdirSync");
  function mkdirSync(path, mode) {
    doOp2("mkdirSync", true, path, normalizeMode(mode, 511), cred);
  }
  __name(mkdirSync, "mkdirSync");
  function readdirSync(path) {
    path = normalizePath(path);
    const entries = doOp2("readdirSync", true, path, cred);
    const points = [...mounts.keys()];
    for (const point of points) {
      if (point.startsWith(path)) {
        const entry = point.slice(path.length);
        if (entry.includes("/") || entry.length == 0) {
          continue;
        }
        entries.push(entry);
      }
    }
    return entries;
  }
  __name(readdirSync, "readdirSync");
  function linkSync(srcpath, dstpath) {
    dstpath = normalizePath(dstpath);
    return doOp2("linkSync", false, srcpath, dstpath, cred);
  }
  __name(linkSync, "linkSync");
  function symlinkSync(srcpath, dstpath, type) {
    if (!["file", "dir", "junction"].includes(type)) {
      throw new ApiError(22 /* EINVAL */, "Invalid type: " + type);
    }
    dstpath = normalizePath(dstpath);
    return doOp2("symlinkSync", false, srcpath, dstpath, type, cred);
  }
  __name(symlinkSync, "symlinkSync");
  function readlinkSync(path) {
    return doOp2("readlinkSync", false, path, cred);
  }
  __name(readlinkSync, "readlinkSync");
  function chownSync(path, uid, gid) {
    doOp2("chownSync", true, path, uid, gid, cred);
  }
  __name(chownSync, "chownSync");
  function lchownSync(path, uid, gid) {
    doOp2("chownSync", false, path, uid, gid, cred);
  }
  __name(lchownSync, "lchownSync");
  function chmodSync(path, mode) {
    const numMode = normalizeMode(mode, -1);
    if (numMode < 0) {
      throw new ApiError(22 /* EINVAL */, `Invalid mode.`);
    }
    doOp2("chmodSync", true, path, numMode, cred);
  }
  __name(chmodSync, "chmodSync");
  function lchmodSync(path, mode) {
    const numMode = normalizeMode(mode, -1);
    if (numMode < 1) {
      throw new ApiError(22 /* EINVAL */, `Invalid mode.`);
    }
    doOp2("chmodSync", false, path, numMode, cred);
  }
  __name(lchmodSync, "lchmodSync");
  function utimesSync(path, atime, mtime) {
    doOp2("utimesSync", true, path, normalizeTime(atime), normalizeTime(mtime), cred);
  }
  __name(utimesSync, "utimesSync");
  function lutimesSync(path, atime, mtime) {
    doOp2("utimesSync", false, path, normalizeTime(atime), normalizeTime(mtime), cred);
  }
  __name(lutimesSync, "lutimesSync");
  function realpathSync(path, cache = {}) {
    path = normalizePath(path);
    const { fs: fs2, path: resolvedPath, mountPoint } = resolveFS(path);
    try {
      const stats = fs2.statSync(resolvedPath, cred);
      if (!stats.isSymbolicLink()) {
        return path;
      }
      const dst = normalizePath(mountPoint + fs2.readlinkSync(resolvedPath, cred));
      return realpathSync(dst);
    } catch (e) {
      throw fixError(e, { [resolvedPath]: path });
    }
  }
  __name(realpathSync, "realpathSync");
  function accessSync(path, mode = 384) {
    return doOp2("accessSync", true, path, mode, cred);
  }
  __name(accessSync, "accessSync");

  // src/emulation/fs.ts
  var fs = emulation_exports;
  var fs_default = fs;

  // src/backends/FileSystemAccess.ts
  var handleError = /* @__PURE__ */ __name((path = "", error) => {
    if (error.name === "NotFoundError") {
      throw ApiError.ENOENT(path);
    }
    throw error;
  }, "handleError");
  var FileSystemAccessFile = class extends PreloadFile {
    constructor(_fs, _path, _flag, _stat, contents) {
      super(_fs, _path, _flag, _stat, contents);
    }
    async sync() {
      if (this.isDirty()) {
        await this._fs._sync(this.getPath(), this.getBuffer(), this.getStats(), Cred.Root);
        this.resetDirty();
      }
    }
    async close() {
      await this.sync();
    }
  };
  __name(FileSystemAccessFile, "FileSystemAccessFile");
  var _FileSystemAccessFileSystem = class extends BaseFileSystem {
    constructor({ handle }) {
      super();
      this._handles = { "/": handle };
    }
    static isAvailable() {
      return typeof FileSystemHandle === "function";
    }
    get metadata() {
      return {
        ...super.metadata,
        name: _FileSystemAccessFileSystem.Name
      };
    }
    async _sync(p, data, stats, cred2) {
      const currentStats = await this.stat(p, cred2);
      if (stats.mtime !== currentStats.mtime) {
        await this.writeFile(p, data, null, FileFlag.getFileFlag("w"), currentStats.mode, cred2);
      }
    }
    async rename(oldPath, newPath, cred2) {
      try {
        const handle = await this.getHandle(oldPath);
        if (handle instanceof FileSystemDirectoryHandle) {
          const files = await this.readdir(oldPath, cred2);
          await this.mkdir(newPath, "wx", cred2);
          if (files.length === 0) {
            await this.unlink(oldPath, cred2);
          } else {
            for (const file of files) {
              await this.rename(join(oldPath, file), join(newPath, file), cred2);
              await this.unlink(oldPath, cred2);
            }
          }
        }
        if (handle instanceof FileSystemFileHandle) {
          const oldFile = await handle.getFile(), destFolder = await this.getHandle(dirname(newPath));
          if (destFolder instanceof FileSystemDirectoryHandle) {
            const newFile = await destFolder.getFileHandle(basename(newPath), { create: true });
            const writable = await newFile.createWritable();
            const buffer = await oldFile.arrayBuffer();
            await writable.write(buffer);
            writable.close();
            await this.unlink(oldPath, cred2);
          }
        }
      } catch (err) {
        handleError(oldPath, err);
      }
    }
    async writeFile(fname, data, encoding, flag, mode, cred2, createFile) {
      const handle = await this.getHandle(dirname(fname));
      if (handle instanceof FileSystemDirectoryHandle) {
        const file = await handle.getFileHandle(basename(fname), { create: true });
        const writable = await file.createWritable();
        await writable.write(data);
        await writable.close();
      }
    }
    async createFile(p, flag, mode, cred2) {
      await this.writeFile(p, Buffer2.alloc(0), null, flag, mode, cred2, true);
      return this.openFile(p, flag, cred2);
    }
    async stat(path, cred2) {
      const handle = await this.getHandle(path);
      if (!handle) {
        throw ApiError.FileError(22 /* EINVAL */, path);
      }
      if (handle instanceof FileSystemDirectoryHandle) {
        return new Stats(FileType.DIRECTORY, 4096);
      }
      if (handle instanceof FileSystemFileHandle) {
        const { lastModified, size } = await handle.getFile();
        return new Stats(FileType.FILE, size, void 0, void 0, lastModified);
      }
    }
    async exists(p, cred2) {
      try {
        await this.getHandle(p);
        return true;
      } catch (e) {
        return false;
      }
    }
    async openFile(path, flags, cred2) {
      const handle = await this.getHandle(path);
      if (handle instanceof FileSystemFileHandle) {
        const file = await handle.getFile();
        const buffer = await file.arrayBuffer();
        return this.newFile(path, flags, buffer, file.size, file.lastModified);
      }
    }
    async unlink(path, cred2) {
      const handle = await this.getHandle(dirname(path));
      if (handle instanceof FileSystemDirectoryHandle) {
        try {
          await handle.removeEntry(basename(path), { recursive: true });
        } catch (e) {
          handleError(path, e);
        }
      }
    }
    async rmdir(path, cred2) {
      return this.unlink(path, cred2);
    }
    async mkdir(p, mode, cred2) {
      const overwrite = mode && mode.flag && mode.flag.includes("w") && !mode.flag.includes("x");
      const existingHandle = await this.getHandle(p);
      if (existingHandle && !overwrite) {
        throw ApiError.EEXIST(p);
      }
      const handle = await this.getHandle(dirname(p));
      if (handle instanceof FileSystemDirectoryHandle) {
        await handle.getDirectoryHandle(basename(p), { create: true });
      }
    }
    async readdir(path, cred2) {
      const handle = await this.getHandle(path);
      if (handle instanceof FileSystemDirectoryHandle) {
        const _keys = [];
        for await (const key of handle.keys()) {
          _keys.push(join(path, key));
        }
        return _keys;
      }
    }
    newFile(path, flag, data, size, lastModified) {
      return new FileSystemAccessFile(this, path, flag, new Stats(FileType.FILE, size || 0, void 0, void 0, lastModified || (/* @__PURE__ */ new Date()).getTime()), Buffer2.from(data));
    }
    async getHandle(path) {
      if (path === "/") {
        return this._handles["/"];
      }
      let walkedPath = "/";
      const [, ...pathParts] = path.split("/");
      const getHandleParts = /* @__PURE__ */ __name(async ([pathPart, ...remainingPathParts]) => {
        const walkingPath = join(walkedPath, pathPart);
        const continueWalk = /* @__PURE__ */ __name((handle2) => {
          walkedPath = walkingPath;
          this._handles[walkedPath] = handle2;
          if (remainingPathParts.length === 0) {
            return this._handles[path];
          }
          getHandleParts(remainingPathParts);
        }, "continueWalk");
        const handle = this._handles[walkedPath];
        try {
          return await continueWalk(await handle.getDirectoryHandle(pathPart));
        } catch (error) {
          if (error.name === "TypeMismatchError") {
            try {
              return await continueWalk(await handle.getFileHandle(pathPart));
            } catch (err) {
              handleError(walkingPath, err);
            }
          } else if (error.message === "Name is not allowed.") {
            throw new ApiError(2 /* ENOENT */, error.message, walkingPath);
          } else {
            handleError(walkingPath, error);
          }
        }
      }, "getHandleParts");
      getHandleParts(pathParts);
    }
  };
  var FileSystemAccessFileSystem = _FileSystemAccessFileSystem;
  __name(FileSystemAccessFileSystem, "FileSystemAccessFileSystem");
  FileSystemAccessFileSystem.Name = "FileSystemAccess";
  FileSystemAccessFileSystem.Create = CreateBackend.bind(_FileSystemAccessFileSystem);
  FileSystemAccessFileSystem.Options = {};

  // src/backends/FolderAdapter.ts
  var _FolderAdapter = class extends BaseFileSystem {
    constructor({ folder, wrapped }) {
      super();
      this._folder = folder;
      this._wrapped = wrapped;
      this._ready = this._initialize();
    }
    static isAvailable() {
      return true;
    }
    get metadata() {
      return { ...super.metadata, ...this._wrapped.metadata, supportsLinks: false };
    }
    /**
     * Initialize the file system. Ensures that the wrapped file system
     * has the given folder.
     */
    async _initialize() {
      const exists3 = await this._wrapped.exists(this._folder, Cred.Root);
      if (!exists3 && this._wrapped.metadata.readonly) {
        throw ApiError.ENOENT(this._folder);
      }
      await this._wrapped.mkdir(this._folder, 511, Cred.Root);
      return this;
    }
  };
  var FolderAdapter = _FolderAdapter;
  __name(FolderAdapter, "FolderAdapter");
  FolderAdapter.Name = "FolderAdapter";
  FolderAdapter.Create = CreateBackend.bind(_FolderAdapter);
  FolderAdapter.Options = {
    folder: {
      type: "string",
      description: "The folder to use as the root directory"
    },
    wrapped: {
      type: "object",
      description: "The file system to wrap"
    }
  };
  function translateError(folder, e) {
    if (e !== null && typeof e === "object") {
      const err = e;
      let p = err.path;
      if (p) {
        p = "/" + relative(folder, p);
        err.message = err.message.replace(err.path, p);
        err.path = p;
      }
    }
    return e;
  }
  __name(translateError, "translateError");
  function wrapCallback(folder, cb) {
    if (typeof cb === "function") {
      return function(err) {
        if (arguments.length > 0) {
          arguments[0] = translateError(folder, err);
        }
        cb.apply(null, arguments);
      };
    } else {
      return cb;
    }
  }
  __name(wrapCallback, "wrapCallback");
  function wrapFunction(name, wrapFirst, wrapSecond) {
    if (name.slice(name.length - 4) !== "Sync") {
      return function() {
        if (arguments.length > 0) {
          if (wrapFirst) {
            arguments[0] = join(this._folder, arguments[0]);
          }
          if (wrapSecond) {
            arguments[1] = join(this._folder, arguments[1]);
          }
          arguments[arguments.length - 1] = wrapCallback(this._folder, arguments[arguments.length - 1]);
        }
        return this._wrapped[name].apply(this._wrapped, arguments);
      };
    } else {
      return function() {
        try {
          if (wrapFirst) {
            arguments[0] = join(this._folder, arguments[0]);
          }
          if (wrapSecond) {
            arguments[1] = join(this._folder, arguments[1]);
          }
          return this._wrapped[name].apply(this._wrapped, arguments);
        } catch (e) {
          throw translateError(this._folder, e);
        }
      };
    }
  }
  __name(wrapFunction, "wrapFunction");
  [
    "diskSpace",
    "stat",
    "statSync",
    "open",
    "openSync",
    "unlink",
    "unlinkSync",
    "rmdir",
    "rmdirSync",
    "mkdir",
    "mkdirSync",
    "readdir",
    "readdirSync",
    "exists",
    "existsSync",
    "realpath",
    "realpathSync",
    "truncate",
    "truncateSync",
    "readFile",
    "readFileSync",
    "writeFile",
    "writeFileSync",
    "appendFile",
    "appendFileSync",
    "chmod",
    "chmodSync",
    "chown",
    "chownSync",
    "utimes",
    "utimesSync",
    "readlink",
    "readlinkSync"
  ].forEach((name) => {
    FolderAdapter.prototype[name] = wrapFunction(name, true, false);
  });
  ["rename", "renameSync", "link", "linkSync", "symlink", "symlinkSync"].forEach((name) => {
    FolderAdapter.prototype[name] = wrapFunction(name, true, true);
  });

  // src/backends/IndexedDB.ts
  var indexedDB = (() => {
    try {
      return globalThis.indexedDB || globalThis.mozIndexedDB || globalThis.webkitIndexedDB || globalThis.msIndexedDB;
    } catch {
      return null;
    }
  })();
  function convertError(e, message = e.toString()) {
    switch (e.name) {
      case "NotFoundError":
        return new ApiError(2 /* ENOENT */, message);
      case "QuotaExceededError":
        return new ApiError(28 /* ENOSPC */, message);
      default:
        return new ApiError(5 /* EIO */, message);
    }
  }
  __name(convertError, "convertError");
  function onErrorHandler(cb, code = 5 /* EIO */, message = null) {
    return function(e) {
      e.preventDefault();
      cb(new ApiError(code, message !== null ? message : void 0));
    };
  }
  __name(onErrorHandler, "onErrorHandler");
  var IndexedDBROTransaction = class {
    constructor(tx, store) {
      this.tx = tx;
      this.store = store;
    }
    get(key) {
      return new Promise((resolve2, reject) => {
        try {
          const r = this.store.get(key);
          r.onerror = onErrorHandler(reject);
          r.onsuccess = (event) => {
            const result = event.target.result;
            if (result === void 0) {
              resolve2(result);
            } else {
              resolve2(Buffer2.from(result));
            }
          };
        } catch (e) {
          reject(convertError(e));
        }
      });
    }
  };
  __name(IndexedDBROTransaction, "IndexedDBROTransaction");
  var IndexedDBRWTransaction = class extends IndexedDBROTransaction {
    constructor(tx, store) {
      super(tx, store);
    }
    /**
     * @todo return false when add has a key conflict (no error)
     */
    put(key, data, overwrite) {
      return new Promise((resolve2, reject) => {
        try {
          const r = overwrite ? this.store.put(data, key) : this.store.add(data, key);
          r.onerror = onErrorHandler(reject);
          r.onsuccess = () => {
            resolve2(true);
          };
        } catch (e) {
          reject(convertError(e));
        }
      });
    }
    del(key) {
      return new Promise((resolve2, reject) => {
        try {
          const r = this.store.delete(key);
          r.onerror = onErrorHandler(reject);
          r.onsuccess = () => {
            resolve2();
          };
        } catch (e) {
          reject(convertError(e));
        }
      });
    }
    commit() {
      return new Promise((resolve2) => {
        setTimeout(resolve2, 0);
      });
    }
    abort() {
      return new Promise((resolve2, reject) => {
        try {
          this.tx.abort();
          resolve2();
        } catch (e) {
          reject(convertError(e));
        }
      });
    }
  };
  __name(IndexedDBRWTransaction, "IndexedDBRWTransaction");
  var IndexedDBStore = class {
    constructor(db, storeName) {
      this.db = db;
      this.storeName = storeName;
    }
    static Create(storeName, indexedDB2) {
      return new Promise((resolve2, reject) => {
        const openReq = indexedDB2.open(storeName, 1);
        openReq.onupgradeneeded = (event) => {
          const db = event.target.result;
          if (db.objectStoreNames.contains(storeName)) {
            db.deleteObjectStore(storeName);
          }
          db.createObjectStore(storeName);
        };
        openReq.onsuccess = (event) => {
          resolve2(new IndexedDBStore(event.target.result, storeName));
        };
        openReq.onerror = onErrorHandler(reject, 13 /* EACCES */);
      });
    }
    name() {
      return IndexedDBFileSystem.Name + " - " + this.storeName;
    }
    clear() {
      return new Promise((resolve2, reject) => {
        try {
          const tx = this.db.transaction(this.storeName, "readwrite"), objectStore = tx.objectStore(this.storeName), r = objectStore.clear();
          r.onsuccess = () => {
            setTimeout(resolve2, 0);
          };
          r.onerror = onErrorHandler(reject);
        } catch (e) {
          reject(convertError(e));
        }
      });
    }
    beginTransaction(type = "readonly") {
      const tx = this.db.transaction(this.storeName, type), objectStore = tx.objectStore(this.storeName);
      if (type === "readwrite") {
        return new IndexedDBRWTransaction(tx, objectStore);
      } else if (type === "readonly") {
        return new IndexedDBROTransaction(tx, objectStore);
      } else {
        throw new ApiError(22 /* EINVAL */, "Invalid transaction type.");
      }
    }
  };
  __name(IndexedDBStore, "IndexedDBStore");
  var _IndexedDBFileSystem = class extends AsyncKeyValueFileSystem {
    static isAvailable(idbFactory = globalThis.indexedDB) {
      try {
        if (!(idbFactory instanceof IDBFactory)) {
          return false;
        }
        const req = indexedDB.open("__browserfs_test__");
        if (!req) {
          return false;
        }
      } catch (e) {
        return false;
      }
    }
    constructor({ cacheSize = 100, storeName = "browserfs", idbFactory = globalThis.indexedDB }) {
      super(cacheSize);
      this._ready = IndexedDBStore.Create(storeName, idbFactory).then((store) => {
        this.init(store);
        return this;
      });
    }
  };
  var IndexedDBFileSystem = _IndexedDBFileSystem;
  __name(IndexedDBFileSystem, "IndexedDBFileSystem");
  IndexedDBFileSystem.Name = "IndexedDB";
  IndexedDBFileSystem.Create = CreateBackend.bind(_IndexedDBFileSystem);
  IndexedDBFileSystem.Options = {
    storeName: {
      type: "string",
      optional: true,
      description: "The name of this file system. You can have multiple IndexedDB file systems operating at once, but each must have a different name."
    },
    cacheSize: {
      type: "number",
      optional: true,
      description: "The size of the inode cache. Defaults to 100. A size of 0 or below disables caching."
    },
    idbFactory: {
      type: "object",
      optional: true,
      description: "The IDBFactory to use. Defaults to globalThis.indexedDB."
    }
  };

  // src/backends/index.ts
  var backends = {
    FileSystemAccess: FileSystemAccessFileSystem,
    FolderAdapter,
    InMemory: InMemoryFileSystem,
    IndexedDB: IndexedDBFileSystem
  };

  // src/index.ts
  if (process_exports && void 0) {
    (void 0)();
  }
  function registerBackend(name, fs2) {
    backends[name] = fs2;
  }
  __name(registerBackend, "registerBackend");
  function initialize2(mounts2, uid = 0, gid = 0) {
    setCred(new Cred(uid, gid, uid, gid, uid, gid));
    return fs_default.initialize(mounts2);
  }
  __name(initialize2, "initialize");
  async function _configure(config2) {
    if ("fs" in config2 || config2 instanceof FileSystem) {
      config2 = { "/": config2 };
    }
    for (let [point, value] of Object.entries(config2)) {
      if (typeof value == "number") {
        continue;
      }
      point = point.toString();
      if (value instanceof FileSystem) {
        continue;
      }
      if (typeof value == "string") {
        value = { fs: value };
      }
      config2[point] = await getFileSystem(value);
    }
    return initialize2(config2);
  }
  __name(_configure, "_configure");
  function configure(config2, cb) {
    if (typeof cb != "function") {
      return _configure(config2);
    }
    _configure(config2).then(() => cb()).catch((err) => cb(err));
    return;
  }
  __name(configure, "configure");
  async function _getFileSystem({ fs: fsName, options = {} }) {
    if (!fsName) {
      throw new ApiError(1 /* EPERM */, 'Missing "fs" property on configuration object.');
    }
    if (typeof options !== "object" || options === null) {
      throw new ApiError(22 /* EINVAL */, 'Invalid "options" property on configuration object.');
    }
    const props = Object.keys(options).filter((k) => k != "fs");
    for (const prop of props) {
      const opt = options[prop];
      if (opt === null || typeof opt !== "object" || !("fs" in opt)) {
        continue;
      }
      const fs2 = await _getFileSystem(opt);
      options[prop] = fs2;
    }
    const fsc = backends[fsName];
    if (!fsc) {
      throw new ApiError(1 /* EPERM */, `File system ${fsName} is not available in BrowserFS.`);
    } else {
      return fsc.Create(options);
    }
  }
  __name(_getFileSystem, "_getFileSystem");
  function getFileSystem(config2, cb) {
    if (typeof cb != "function") {
      return _getFileSystem(config2);
    }
    _getFileSystem(config2).then((fs2) => cb(null, fs2)).catch((err) => cb(err));
    return;
  }
  __name(getFileSystem, "getFileSystem");
  var src_default = fs_default;
  return __toCommonJS(src_exports);
})();
/*! Bundled license information:

@jspm/core/nodelibs/browser/buffer.js:
  (*! ieee754. BSD-3-Clause License. Feross Aboukhadijeh <https://feross.org/opensource> *)
*/
//# sourceMappingURL=index.js.map
