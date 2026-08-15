/// 平台门面：原生平台（支持 dart:ffi）导出 FFI 实现，Web 导出 no-op stub。
library;

export 'rust_bridge_stub.dart' if (dart.library.ffi) 'rust_bridge_io.dart';
