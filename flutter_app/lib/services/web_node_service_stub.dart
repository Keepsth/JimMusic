class WebNodeService {
  bool get available => false;

  Future<Map<String, dynamic>?> start() async => null;
  Future<Map<String, dynamic>?> status() async => null;
  Future<Map<String, dynamic>?> connect(String address) async => null;
  Future<void> stop() async {}
}
