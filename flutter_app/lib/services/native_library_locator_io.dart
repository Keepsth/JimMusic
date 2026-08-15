import 'dart:io';

/// Resolves a Rust cdylib shipped beside a desktop Flutter application.
/// Android/HarmonyOS use the platform linker name. iOS resolves the statically
/// linked symbols through `DynamicLibrary.process` before this helper is called.
String resolveBundledLibrary(String fileName) {
  if (Platform.isAndroid || Platform.isIOS) return fileName;
  final executableDirectory = File(Platform.resolvedExecutable).parent;
  final candidates = <File>[
    if (Platform.isLinux) File('${executableDirectory.path}/lib/$fileName'),
    if (Platform.isMacOS)
      File('${executableDirectory.parent.path}/Frameworks/$fileName'),
    File('${executableDirectory.path}/$fileName'),
    File(fileName),
  ];
  for (final candidate in candidates) {
    if (candidate.existsSync()) return candidate.absolute.path;
  }
  return fileName;
}
