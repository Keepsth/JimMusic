import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../providers/audio_output_provider.dart';
import '../providers/control_plane_provider.dart';
import '../providers/music_player_provider.dart';
import '../services/control_api_types.dart' show networkPauseHint;
import '../widgets/community_native_confirm.dart';
import '../widgets/plugin_config_form.dart';
import '../widgets/publish_wizard.dart';

class ControlCenterScreen extends StatelessWidget {
  const ControlCenterScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return DefaultTabController(
      length: 7,
      child: Scaffold(
        appBar: AppBar(
          title: const Text('JimMusic 控制台'),
          actions: [
            Consumer<ControlPlaneProvider>(
              builder: (context, control, _) {
                if (!control.loading) return const SizedBox.shrink();
                // UI-010：取消当前进行中的操作。
                return IconButton(
                  tooltip: '取消当前操作',
                  onPressed: control.cancelCurrentOperation,
                  icon: const Icon(Icons.stop_circle_outlined),
                );
              },
            ),
            IconButton(
              tooltip: '连接设置',
              onPressed: () => _configure(context),
              icon: const Icon(Icons.link),
            ),
            IconButton(
              tooltip: '刷新',
              onPressed: context.read<ControlPlaneProvider>().refresh,
              icon: const Icon(Icons.refresh),
            ),
          ],
          bottom: const TabBar(
            isScrollable: true,
            tabs: [
              Tab(text: '节点'),
              Tab(text: '传输'),
              Tab(text: '曲库'),
              Tab(text: '发布'),
              Tab(text: '社区'),
              Tab(text: '插件'),
              Tab(text: 'Audio Path'),
            ],
          ),
        ),
        body: Consumer<ControlPlaneProvider>(
          builder: (context, control, _) => Column(
            children: [
              if (control.loading) const LinearProgressIndicator(),
              if (control.error != null)
                MaterialBanner(
                  content: Text(
                    control.userErrorText.isNotEmpty
                        ? control.userErrorText
                        : control.error!,
                  ),
                  leading: const Icon(Icons.error_outline),
                  actions: [
                    TextButton(
                      onPressed: control.clearError,
                      child: const Text('关闭'),
                    ),
                  ],
                ),
              Expanded(
                child: TabBarView(
                  children: [
                    _NodeTab(control),
                    _TransfersTab(control),
                    _LibraryTab(control),
                    _PublishTab(control),
                    _CommunityTab(control),
                    _PluginsTab(control),
                    _AudioPathTab(control),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Future<void> _configure(BuildContext context) async {
    final control = context.read<ControlPlaneProvider>();
    final endpoint = TextEditingController(text: control.endpoint);
    final token = TextEditingController(text: control.token);
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('控制面连接'),
        content: SizedBox(
          width: 480,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: endpoint,
                decoration: const InputDecoration(
                  labelText: '地址',
                  hintText: 'http://127.0.0.1:8787/v1',
                ),
              ),
              TextField(
                controller: token,
                obscureText: true,
                decoration: const InputDecoration(
                  labelText: 'Bearer Token（仅本次运行，不落盘）',
                ),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext, true),
            child: const Text('连接'),
          ),
        ],
      ),
    );
    if (accepted == true) await control.configure(endpoint.text, token.text);
  }
}

class _NodeTab extends StatelessWidget {
  final ControlPlaneProvider control;
  const _NodeTab(this.control);

  @override
  Widget build(BuildContext context) {
    final deviceNode = control.deviceNode;
    final serviceNode = control.node;
    final node = deviceNode ?? serviceNode;
    if (node == null) return const _ConnectHint();
    final limitations = (node['limitations'] as List<dynamic>? ?? const []).map(
      (e) => '$e',
    );
    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
        _StatusCard(
          title: deviceNode == null ? '控制服务原生节点' : '应用内原生节点',
          status: '${node['lifecycle_state'] ?? 'unknown'}',
          rows: {
            'Peer ID': '${node['peer_id'] ?? '-'}',
            '路由': '${node['routing_status'] ?? '-'}',
            '连接节点': '${node['connected_peers'] ?? 0}',
            '传输': _joined(node['transports']),
            '监听地址': _joined(node['listen_addresses']),
            '仓库': _bytes(node['repository_bytes']),
            '缓存': _bytes(node['cache_bytes']),
            '固定内容': _bytes(node['pinned_bytes']),
            '上行 / 下行':
                '${_bytes(node['bytes_up'])} / ${_bytes(node['bytes_down'])}',
          },
        ),
        if (deviceNode != null && serviceNode != null) ...[
          const SizedBox(height: 12),
          _StatusCard(
            title: '控制服务原生节点',
            status: '${serviceNode['lifecycle_state'] ?? 'unknown'}',
            rows: {
              'Peer ID': '${serviceNode['peer_id'] ?? '-'}',
              '路由': '${serviceNode['routing_status'] ?? '-'}',
              '连接节点': '${serviceNode['connected_peers'] ?? 0}',
              '传输': _joined(serviceNode['transports']),
              '监听地址': _joined(serviceNode['listen_addresses']),
              '仓库': _bytes(serviceNode['repository_bytes']),
              '缓存': _bytes(serviceNode['cache_bytes']),
              '固定内容': _bytes(serviceNode['pinned_bytes']),
              '上行 / 下行':
                  '${_bytes(serviceNode['bytes_up'])} / ${_bytes(serviceNode['bytes_down'])}',
            },
          ),
        ],
        if (control.browserNode case final browserNode?) ...[
          const SizedBox(height: 12),
          _StatusCard(
            title: '浏览器 Helia 节点',
            status: '${browserNode['lifecycle_state'] ?? 'unknown'}',
            rows: {
              'Peer ID': '${browserNode['peer_id'] ?? '-'}',
              '存储': '${browserNode['storage'] ?? 'IndexedDB'}',
              '路由': '${browserNode['routing_status'] ?? '-'}',
              '连接节点': '${browserNode['connected_peers'] ?? 0}',
              '传输': _joined(browserNode['transports']),
              '监听地址': _joined(browserNode['listen_addresses']),
            },
          ),
        ],
        const SizedBox(height: 12),
        Wrap(
          spacing: 12,
          runSpacing: 8,
          children: [
            FilledButton.icon(
              onPressed: control.nodeConfig == null
                  ? null
                  : () => _configureNode(context),
              icon: const Icon(Icons.tune),
              label: const Text('节点与网络策略'),
            ),
            OutlinedButton.icon(
              onPressed: deviceNode != null || serviceNode != null
                  ? () => _connectPeer(context, browser: false)
                  : null,
              icon: const Icon(Icons.hub_outlined),
              label: const Text('连接原生节点'),
            ),
            if (control.browserNode != null)
              OutlinedButton.icon(
                onPressed: () => _connectPeer(context, browser: true),
                icon: const Icon(Icons.language),
                label: const Text('连接浏览器节点'),
              ),
            OutlinedButton.icon(
              onPressed: control.connected ? () => _pin(context) : null,
              icon: const Icon(Icons.push_pin_outlined),
              label: const Text('固定 CID'),
            ),
            OutlinedButton.icon(
              onPressed: control.connected ? () => _diagnostics(context) : null,
              icon: const Icon(Icons.health_and_safety_outlined),
              label: const Text('安全诊断快照'),
            ),
          ],
        ),
        if (control.pins.isNotEmpty) ...[
          const SizedBox(height: 12),
          ExpansionTile(
            title: Text('本机 Pin（${control.pins.length}）'),
            children: control.pins.map((entry) {
              final cid = '${entry['cid']}';
              final health =
                  entry['health'] as Map<String, dynamic>? ?? const {};
              return ListTile(
                title: SelectableText(cid),
                subtitle: Text(
                  '${health['health'] ?? 'unknown'} · providers ${health['observed_providers'] ?? 0}',
                ),
                trailing: IconButton(
                  tooltip: '取消 Pin',
                  onPressed: () => control.unpin(cid),
                  icon: const Icon(Icons.remove_circle_outline),
                ),
              );
            }).toList(),
          ),
        ],
        if (limitations.isNotEmpty) ...[
          const SizedBox(height: 12),
          Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text(
                    '当前限制',
                    style: TextStyle(fontWeight: FontWeight.bold),
                  ),
                  const SizedBox(height: 8),
                  ...limitations.map((text) => Text('• $text')),
                ],
              ),
            ),
          ),
        ],
      ],
    );
  }

  static String _joined(Object? value) {
    if (value is! List<dynamic> || value.isEmpty) return '-';
    return value.map((entry) => '$entry').join(', ');
  }

  Future<void> _connectPeer(
    BuildContext context, {
    required bool browser,
  }) async {
    final address = TextEditingController();
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text(browser ? '连接浏览器节点' : '连接原生节点'),
        content: TextField(
          controller: address,
          decoration: const InputDecoration(
            labelText: 'libp2p Multiaddr',
            hintText: '/ip4/127.0.0.1/tcp/4001/ws/p2p/12D3Koo…',
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext, true),
            child: const Text('连接'),
          ),
        ],
      ),
    );
    if (accepted != true || address.text.trim().isEmpty) return;
    try {
      if (browser) {
        await control.connectBrowserPeer(address.text);
      } else {
        await control.connectNativePeer(address.text);
      }
    } catch (error) {
      if (context.mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('连接失败：$error')));
      }
    }
  }

  Future<void> _pin(BuildContext context) async {
    final cid = TextEditingController();
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('固定本机已有对象'),
        content: TextField(
          controller: cid,
          decoration: const InputDecoration(labelText: 'CID'),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext, true),
            child: const Text('Pin'),
          ),
        ],
      ),
    );
    if (accepted == true && cid.text.trim().isNotEmpty) {
      await control.pin(cid.text);
    }
  }

  Future<void> _diagnostics(BuildContext context) async {
    Map<String, dynamic> report;
    try {
      report = await control.diagnostics();
    } catch (error) {
      if (context.mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('诊断快照失败：$error')));
      }
      return;
    }
    if (!context.mounted) return;
    final encoded = const JsonEncoder.withIndent('  ').convert(report);
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('安全诊断快照'),
        content: SizedBox(
          width: 720,
          height: 480,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('已排除令牌、身份私钥、口令、媒体路径、插件配置和制品路径。'),
              const SizedBox(height: 8),
              Expanded(
                child: SingleChildScrollView(child: SelectableText(encoded)),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () async {
              await Clipboard.setData(ClipboardData(text: encoded));
              if (dialogContext.mounted) Navigator.pop(dialogContext);
            },
            child: const Text('复制 JSON'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text('关闭'),
          ),
        ],
      ),
    );
  }

  Future<void> _configureNode(BuildContext context) async {
    final config = control.nodeConfig!;
    final storage = TextEditingController(
      text: '${config['storage_limit_bytes']}',
    );
    final cache = TextEditingController(text: '${config['cache_limit_bytes']}');
    final concurrency = TextEditingController(
      text: '${config['max_concurrent_transfers']}',
    );
    final download = TextEditingController(
      text: '${config['download_limit_bytes_per_second'] ?? ''}',
    );
    var metered = config['metered_network_allowed'] == true;
    var networkClass = (config['network_class'] as String?) ?? 'unknown';
    var assistPin = config['assist_pin_favorites'] == true;
    var autoReplicate = config['auto_replicate_published'] == true;
    final pinServices = TextEditingController(
      text: (config['pin_services'] as List<dynamic>? ?? const [])
          .whereType<String>()
          .join(', '),
    );
    String? validationError;
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: const Text('节点与网络策略'),
          content: SizedBox(
            width: 520,
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  TextField(
                    controller: storage,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(labelText: '仓库上限（bytes）'),
                  ),
                  TextField(
                    controller: cache,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(labelText: '缓存上限（bytes）'),
                  ),
                  TextField(
                    controller: concurrency,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(
                      labelText: '最大并发传输（1–64）',
                    ),
                  ),
                  const ListTile(
                    contentPadding: EdgeInsets.zero,
                    leading: Icon(Icons.info_outline),
                    title: Text('上传限速暂不支持'),
                    subtitle: Text(
                      '内嵌 Bitswap 服务没有带宽节流能力，设置会被结构化拒绝'
                      '（unsupported）。上传目前不限速。',
                    ),
                  ),
                  TextField(
                    controller: download,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(
                      labelText: '下载限速 bytes/s（留空不限）',
                    ),
                  ),
                  SwitchListTile(
                    title: const Text('允许计量网络'),
                    value: metered,
                    onChanged: (value) => setState(() => metered = value),
                  ),
                  SwitchListTile(
                    title: const Text('收藏时协助 Pin'),
                    subtitle: const Text('收藏曲目时帮助固定其内容 CID（DST-009）'),
                    value: assistPin,
                    onChanged: (value) => setState(() => assistPin = value),
                  ),
                  SwitchListTile(
                    title: const Text('发布后自动复刻'),
                    subtitle: const Text('发布成功后自动复刻各 rendition 内容（DST-010）'),
                    value: autoReplicate,
                    onChanged: (value) => setState(() => autoReplicate = value),
                  ),
                  TextField(
                    controller: pinServices,
                    decoration: const InputDecoration(
                      labelText: '第三方 Pin 服务（Kubo 兼容，逗号分隔）',
                      hintText: 'https://pin.example.com',
                    ),
                  ),
                  DropdownButtonFormField<String>(
                    initialValue: networkClass,
                    decoration: const InputDecoration(labelText: '当前网络类别'),
                    items: const [
                      DropdownMenuItem(value: 'wifi', child: Text('Wi-Fi')),
                      DropdownMenuItem(value: 'cellular', child: Text('蜂窝网络')),
                      DropdownMenuItem(value: 'ethernet', child: Text('有线网络')),
                      DropdownMenuItem(value: 'unknown', child: Text('未声明')),
                    ],
                    onChanged: (value) =>
                        setState(() => networkClass = value ?? 'unknown'),
                  ),
                  if (validationError != null)
                    Text(
                      validationError!,
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(dialogContext, false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () {
                final storageValue = int.tryParse(storage.text);
                final cacheValue = int.tryParse(cache.text);
                final concurrencyValue = int.tryParse(concurrency.text);
                final downloadValue = download.text.trim().isEmpty
                    ? 1
                    : int.tryParse(download.text);
                if (storageValue == null ||
                    storageValue <= 0 ||
                    cacheValue == null ||
                    cacheValue < 0 ||
                    cacheValue > storageValue ||
                    concurrencyValue == null ||
                    concurrencyValue < 1 ||
                    concurrencyValue > 64 ||
                    downloadValue == null ||
                    downloadValue <= 0) {
                  setState(() => validationError = '请检查仓库、缓存和并发范围');
                  return;
                }
                Navigator.pop(dialogContext, true);
              },
              child: const Text('保存'),
            ),
          ],
        ),
      ),
    );
    if (accepted != true) return;
    await control.configureNode({
      'storage_limit_bytes': int.parse(storage.text),
      'cache_limit_bytes': int.parse(cache.text),
      'max_concurrent_transfers': int.parse(concurrency.text),
      'upload_limit_bytes_per_second': null,
      'download_limit_bytes_per_second': download.text.trim().isEmpty
          ? null
          : int.parse(download.text),
      'metered_network_allowed': metered,
      'network_class': networkClass,
      'assist_pin_favorites': assistPin,
      'auto_replicate_published': autoReplicate,
      'pin_services': pinServices.text.trim().isEmpty
          ? const <String>[]
          : pinServices.text
                .split(',')
                .map((value) => value.trim())
                .where((value) => value.isNotEmpty)
                .toList(),
    });
  }
}

/// 曲库统一同步页（PLR-001/PLR-002/PLR-009/UI-002）。
class _LibraryTab extends StatefulWidget {
  final ControlPlaneProvider control;
  const _LibraryTab(this.control);

  @override
  State<_LibraryTab> createState() => _LibraryTabState();
}

class _LibraryTabState extends State<_LibraryTab> {
  ControlPlaneProvider get control => widget.control;
  String? _musicDirectory;

  @override
  void initState() {
    super.initState();
    _loadDirectory();
  }

  Future<void> _loadDirectory() async {
    final directory = await control.musicDirectory();
    if (mounted) {
      setState(() => _musicDirectory = directory);
    }
  }

  Future<void> _setMusicDirectory(BuildContext context) async {
    final controller = TextEditingController(text: _musicDirectory ?? '');
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('设置音乐目录'),
        content: SizedBox(
          width: 480,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: controller,
                decoration: const InputDecoration(
                  labelText: '绝对路径（仅切换，不复制/移动文件）',
                ),
              ),
              const SizedBox(height: 8),
              const Text(
                '“复制”与“移动”选项尚未实现；切换后旧目录中的文件不会自动移动。',
                style: TextStyle(fontSize: 12),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext, true),
            child: const Text('保存'),
          ),
        ],
      ),
    );
    if (accepted != true) return;
    await control.setMusicDirectory(controller.text);
    await _loadDirectory();
  }

  @override
  Widget build(BuildContext context) {
    final player = context.watch<MusicPlayerProvider>();
    final report = control.librarySyncReport;
    final localFiles = player.library
        .where((music) => music.filePath != null && music.filePath!.isNotEmpty)
        .length;
    final network = player.library.length - localFiles;
    return Scaffold(
      floatingActionButton: FloatingActionButton.extended(
        onPressed: control.loading
            ? null
            : () async {
                final messenger = ScaffoldMessenger.of(context);
                final result = await control.syncLibrary(player);
                messenger.showSnackBar(
                  SnackBar(
                    content: Text(
                      result == null
                          ? '同步失败：${control.error ?? '未知错误'}'
                          : '同步完成：$result',
                    ),
                  ),
                );
              },
        icon: const Icon(Icons.sync),
        label: const Text('同步曲库'),
      ),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          Card(
            child: ListTile(
              title: const Text('本地曲库（Flutter）'),
              subtitle: Text(
                '共 ${player.library.length} 首'
                '（本地文件 $localFiles · 网络内容 $network）'
                '\n歌单 ${player.playlists.length} · 收藏 ${player.favoriteIds.length}',
              ),
            ),
          ),
          const SizedBox(height: 8),
          Card(
            child: ListTile(
              leading: const Icon(Icons.folder),
              title: Text('音乐目录：${_musicDirectory ?? '未设置'}'),
              subtitle: const Text(
                '设置后后端扫描/曲库以该目录为默认；当前仅支持“仅切换”'
                '（复制/移动选项未实现）。',
              ),
              trailing: IconButton(
                tooltip: '设置音乐目录',
                onPressed: () => _setMusicDirectory(context),
                icon: const Icon(Icons.edit),
              ),
            ),
          ),
          const SizedBox(height: 8),
          const ListTile(
            leading: Icon(Icons.info_outline),
            title: Text('同步行为'),
            subtitle: Text(
              '本地优先：推送本地文件曲目到控制面；拉取 Manifest/社区曲目'
              '合并进本列表；收藏与命名歌单双向合并；本地无活动会话时'
              '从控制面恢复播放位置（绝不自动播放）。',
            ),
          ),
          if (report != null) ...[
            const SizedBox(height: 8),
            Card(
              child: ListTile(
                leading: Icon(
                  report.errors.isEmpty
                      ? Icons.check_circle
                      : Icons.warning_amber,
                ),
                title: const Text('最近同步'),
                subtitle: Text('$report'),
              ),
            ),
            for (final error in report.errors.take(5))
              ListTile(
                dense: true,
                title: Text(
                  error,
                  style: TextStyle(
                    color: Theme.of(context).colorScheme.error,
                    fontSize: 12,
                  ),
                ),
              ),
          ],
        ],
      ),
    );
  }
}

class _TransfersTab extends StatelessWidget {
  final ControlPlaneProvider control;
  const _TransfersTab(this.control);

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => _create(context),
        icon: const Icon(Icons.add),
        label: const Text('新建传输'),
      ),
      body: control.transfers.isEmpty
          ? const Center(child: Text('暂无传输任务'))
          : ListView.builder(
              padding: const EdgeInsets.only(bottom: 88),
              itemCount: control.transfers.length,
              itemBuilder: (context, index) {
                final task = control.transfers[index];
                final id = '${task['task_id']}';
                final completed =
                    (task['bytes_completed'] as num?)?.toDouble() ?? 0;
                final total = (task['bytes_total'] as num?)?.toDouble();
                final progress = total == null || total <= 0
                    ? null
                    : (completed / total).clamp(0.0, 1.0);
                final state = '${task['state'] ?? 'unknown'}';
                final providers =
                    (task['providers'] as List<dynamic>? ?? const []).join(
                      ', ',
                    );
                final error = task['error'] as Map<String, dynamic>?;
                final destination = '${task['destination'] ?? ''}';
                final streamMime = _audioMimeForPath(destination);
                final canStream =
                    streamMime != null &&
                    !const {
                      'completed',
                      'failed',
                      'cancelled',
                      'integrity_failed',
                    }.contains(state);
                return Card(
                  margin: const EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 6,
                  ),
                  child: ListTile(
                    title: Text('${task['kind']} · ${task['target_cid']}'),
                    subtitle: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '$state · ${_bytes(task['bytes_completed'])}'
                          '${total == null ? '' : ' / ${_bytes(total)}'}'
                          ' · ${_bytes(task['speed_bytes_per_second'])}/s'
                          ' · 优先级 ${task['priority'] ?? 0}',
                        ),
                        Text(
                          'Provider: ${providers.isEmpty ? '-' : providers}',
                        ),
                        if (task['destination'] != null)
                          Text('保存到：${task['destination']}'),
                        if (state == 'verifying' || state == 'committing')
                          Text(state == 'verifying' ? '正在校验 CID' : '正在原子提交'),
                        if (error != null)
                          Text(
                            '${error['code']}: ${error['message']}'
                            '${networkPauseHint('${error['code']}') == null ? '' : '\n提示：${networkPauseHint('${error['code']}')}'}',
                            style: TextStyle(
                              color: Theme.of(context).colorScheme.error,
                            ),
                          ),
                        if (progress != null)
                          LinearProgressIndicator(value: progress),
                      ],
                    ),
                    trailing: PopupMenuButton<String>(
                      onSelected: (action) {
                        if (action.startsWith('priority:')) {
                          control.setTransferPriority(
                            id,
                            int.parse(action.substring('priority:'.length)),
                          );
                        } else if (action == 'stream') {
                          // 边下边播（DST-007）：交给播放器经传输流端点播放。
                          context
                              .read<MusicPlayerProvider>()
                              .playTransferStream(
                                taskId: id,
                                endpoint: control.endpoint,
                                token: control.token,
                                mimeType: streamMime,
                                title: destination.split('/').last,
                              );
                        } else {
                          control.transferAction(id, action);
                        }
                      },
                      itemBuilder: (_) => [
                        if (canStream)
                          const PopupMenuItem(
                            value: 'stream',
                            child: Text('边下边播'),
                          ),
                        const PopupMenuItem(
                          value: 'priority:10',
                          child: Text('设为高优先级'),
                        ),
                        PopupMenuItem(
                          value: 'priority:0',
                          child: Text('设为普通优先级'),
                        ),
                        PopupMenuItem(
                          value: 'priority:-10',
                          child: Text('设为低优先级'),
                        ),
                        PopupMenuDivider(),
                        PopupMenuItem(value: 'pause', child: Text('暂停')),
                        PopupMenuItem(value: 'resume', child: Text('继续')),
                        PopupMenuItem(value: 'cancel', child: Text('取消')),
                        PopupMenuItem(value: 'retry', child: Text('重试失败任务')),
                      ],
                    ),
                  ),
                );
              },
            ),
    );
  }

  Future<void> _create(BuildContext context) async {
    final cid = TextEditingController();
    final destination = TextEditingController();
    String kind = 'fetch';
    int priority = 0;
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: const Text('新建传输'),
          content: SizedBox(
            width: 480,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                DropdownButtonFormField<String>(
                  initialValue: kind,
                  items:
                      const [
                            'fetch',
                            'download',
                            'pin',
                            'publish',
                            'plugin',
                            'report',
                          ]
                          .map(
                            (value) => DropdownMenuItem(
                              value: value,
                              child: Text(value),
                            ),
                          )
                          .toList(),
                  onChanged: (value) => setState(() => kind = value ?? kind),
                  decoration: const InputDecoration(labelText: '任务类型'),
                ),
                DropdownButtonFormField<int>(
                  initialValue: priority,
                  items: const [
                    DropdownMenuItem(value: 10, child: Text('高（10）')),
                    DropdownMenuItem(value: 0, child: Text('普通（0）')),
                    DropdownMenuItem(value: -10, child: Text('低（-10）')),
                  ],
                  onChanged: (value) =>
                      setState(() => priority = value ?? priority),
                  decoration: const InputDecoration(labelText: '调度优先级'),
                ),
                TextField(
                  controller: cid,
                  decoration: const InputDecoration(labelText: '目标 CID'),
                ),
                TextField(
                  controller: destination,
                  decoration: const InputDecoration(labelText: '目标路径（可选）'),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(dialogContext, false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(dialogContext, true),
              child: const Text('创建'),
            ),
          ],
        ),
      ),
    );
    if (accepted == true && cid.text.trim().isNotEmpty) {
      await control.createTransfer(
        cid: cid.text,
        kind: kind,
        destination: destination.text,
        priority: priority,
      );
    }
  }
}

class _PublishTab extends StatelessWidget {
  final ControlPlaneProvider control;
  const _PublishTab(this.control);

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
        const Text(
          '身份私钥始终以加密包保存；本机签名时只在守护进程内短暂解锁，不会写入日志或响应。',
          style: TextStyle(fontSize: 13),
        ),
        const SizedBox(height: 16),
        FilledButton.icon(
          onPressed: () => _identityVaultAction(context, 'generate'),
          icon: const Icon(Icons.key),
          label: const Text('生成并加密导出发布者身份'),
        ),
        const SizedBox(height: 12),
        OutlinedButton.icon(
          onPressed: () => _identityVaultAction(context, 'import'),
          icon: const Icon(Icons.file_download_outlined),
          label: const Text('导入加密身份包'),
        ),
        const SizedBox(height: 12),
        OutlinedButton.icon(
          onPressed: () => _identityVaultAction(context, 'rotate'),
          icon: const Icon(Icons.rotate_right),
          label: const Text('轮换身份密钥'),
        ),
        const SizedBox(height: 12),
        OutlinedButton.icon(
          onPressed: () => _identityVaultAction(context, 'revoke'),
          icon: const Icon(Icons.block),
          label: const Text('签名撤销身份'),
        ),
        const Divider(height: 32),
        FilledButton.icon(
          onPressed: () => _registerIdentity(context),
          icon: const Icon(Icons.badge_outlined),
          label: const Text('登记已签名发布者身份'),
        ),
        const SizedBox(height: 12),
        FilledButton.icon(
          onPressed: () => _signAndPublish(context),
          icon: const Icon(Icons.publish),
          label: const Text('本机解锁、签名并发布'),
        ),
        const SizedBox(height: 12),
        OutlinedButton.icon(
          onPressed: () => _publishWizard(context),
          icon: const Icon(Icons.auto_awesome),
          label: const Text('发布向导（元数据 / rendition 表单）'),
        ),
        const SizedBox(height: 12),
        OutlinedButton.icon(
          onPressed: () => _publish(context),
          icon: const Icon(Icons.verified_outlined),
          label: const Text('提交外部已签名对象'),
        ),
      ],
    );
  }

  Future<void> _registerIdentity(BuildContext context) async {
    final value = await _jsonDialog(context, '发布者身份 JSON', const ['identity']);
    if (value != null) await control.registerIdentity(value['identity']!);
  }

  Future<void> _identityVaultAction(BuildContext context, String action) async {
    final name = TextEditingController();
    final passphrase = TextEditingController();
    final bundle = TextEditingController();
    String? validationError;
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: Text(
            {
                  'generate': '生成发布者身份',
                  'import': '导入发布者身份',
                  'rotate': '轮换发布者身份',
                  'revoke': '撤销发布者身份',
                }[action] ??
                action,
          ),
          content: SizedBox(
            width: 560,
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  TextField(
                    controller: name,
                    decoration: const InputDecoration(labelText: '显示名称'),
                  ),
                  TextField(
                    controller: passphrase,
                    obscureText: true,
                    decoration: const InputDecoration(
                      labelText: '加密口令（至少 10 个字符）',
                    ),
                  ),
                  if (action != 'generate')
                    TextField(
                      controller: bundle,
                      minLines: 6,
                      maxLines: 12,
                      decoration: const InputDecoration(
                        labelText: 'EncryptedIdentityBundleV1 JSON',
                        border: OutlineInputBorder(),
                      ),
                    ),
                  if (validationError != null)
                    Text(
                      validationError!,
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(dialogContext, false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () {
                if (name.text.trim().isEmpty || passphrase.text.length < 10) {
                  setState(() => validationError = '请填写名称和至少 10 个字符的口令');
                  return;
                }
                if (action != 'generate') {
                  try {
                    jsonDecode(bundle.text) as Map<String, dynamic>;
                  } catch (error) {
                    setState(() => validationError = '身份包 JSON 无效：$error');
                    return;
                  }
                }
                Navigator.pop(dialogContext, true);
              },
              child: const Text('执行'),
            ),
          ],
        ),
      ),
    );
    if (accepted != true) return;
    final parsedBundle = action == 'generate'
        ? null
        : jsonDecode(bundle.text) as Map<String, dynamic>;
    switch (action) {
      case 'generate':
        await control.generateIdentity(name.text, passphrase.text);
        break;
      case 'import':
        await control.importIdentity(name.text, passphrase.text, parsedBundle!);
        break;
      case 'rotate':
        await control.rotateIdentity(name.text, passphrase.text, parsedBundle!);
        break;
      case 'revoke':
        await control.revokeIdentity(name.text, passphrase.text, parsedBundle!);
        break;
    }
    if (!context.mounted ||
        control.error != null ||
        control.lastResult == null) {
      return;
    }
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('身份操作完成'),
        content: SizedBox(
          width: 640,
          child: SingleChildScrollView(
            child: SelectableText(
              const JsonEncoder.withIndent('  ').convert(control.lastResult),
            ),
          ),
        ),
        actions: [
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text('完成'),
          ),
        ],
      ),
    );
  }

  /// UI-004：结构化发布向导——元数据/rendition 表单生成 Manifest，
  /// 本机签名发布后展示回执与副本健康度（副本向导）。
  Future<void> _publishWizard(BuildContext context) async {
    if (!context.mounted) return;
    final result =
        await showDialog<
          (Map<String, dynamic>, String, String, Map<String, dynamic>)
        >(context: context, builder: (_) => const PublishWizardDialog());
    if (result == null) return;
    final (manifest, displayName, passphrase, bundle) = result;
    await control.signPublication(
      displayName: displayName,
      passphrase: passphrase,
      bundle: bundle,
      operation: 'publish',
      manifest: manifest,
    );
    if (!context.mounted ||
        control.error != null ||
        control.lastResult == null) {
      return;
    }
    final receipt = control.lastResult;
    final receiptMap = receipt is Map<String, dynamic>
        ? receipt
        : const <String, dynamic>{};
    final manifestCid =
        (receiptMap['receipt'] as Map<String, dynamic>?)?['manifest_cid'];
    String healthLine = '副本健康度未查询';
    if (manifestCid != null) {
      for (final entry in control.pins) {
        if (entry['cid'] == manifestCid) {
          final health = entry['health'] as Map<String, dynamic>?;
          healthLine =
              '本机 Pin: ${health?['local_pin'] ?? '-'} · '
              'Provider 数: ${health?['observed_providers'] ?? '-'} · '
              '状态: ${health?['health'] ?? '-'} · '
              '第三方服务: ${(health?['configured_pin_services'] as List<dynamic>? ?? const []).length}';
          break;
        }
      }
    }
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('发布完成'),
        content: SizedBox(
          width: 680,
          child: SingleChildScrollView(
            child: SelectableText(
              '${const JsonEncoder.withIndent('  ').convert(control.lastResult)}'
              '\n\n副本健康度：$healthLine',
            ),
          ),
        ),
        actions: [
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text('完成'),
          ),
        ],
      ),
    );
  }

  Future<void> _publish(BuildContext context) async {
    final value = await _jsonDialog(context, '发布签名对象', const [
      'manifest',
      'event',
    ]);
    if (value != null) {
      await control.publish(value['manifest']!, value['event']!);
    }
  }

  Future<void> _signAndPublish(BuildContext context) async {
    final name = TextEditingController();
    final passphrase = TextEditingController();
    final bundle = TextEditingController();
    final manifest = TextEditingController();
    final targetCid = TextEditingController();
    final reason = TextEditingController();
    var operation = 'publish';
    String? validationError;
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: const Text('签名发布'),
          content: SizedBox(
            width: 680,
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  TextField(
                    controller: name,
                    decoration: const InputDecoration(labelText: '发布者显示名称'),
                  ),
                  TextField(
                    controller: passphrase,
                    obscureText: true,
                    decoration: const InputDecoration(labelText: '身份包口令'),
                  ),
                  DropdownButtonFormField<String>(
                    initialValue: operation,
                    items: const [
                      DropdownMenuItem(value: 'publish', child: Text('首次发布')),
                      DropdownMenuItem(value: 'update', child: Text('更新发布')),
                      DropdownMenuItem(
                        value: 'tombstone',
                        child: Text('撤回索引（Tombstone）'),
                      ),
                    ],
                    onChanged: (value) =>
                        setState(() => operation = value ?? operation),
                    decoration: const InputDecoration(labelText: '操作'),
                  ),
                  TextField(
                    controller: bundle,
                    minLines: 5,
                    maxLines: 10,
                    decoration: const InputDecoration(
                      labelText: 'EncryptedIdentityBundleV1 JSON',
                      border: OutlineInputBorder(),
                    ),
                  ),
                  if (operation != 'tombstone')
                    TextField(
                      controller: manifest,
                      minLines: 8,
                      maxLines: 16,
                      decoration: const InputDecoration(
                        labelText: 'MusicManifestV1 JSON（签名字段可为空）',
                        border: OutlineInputBorder(),
                      ),
                    ),
                  if (operation == 'tombstone')
                    TextField(
                      controller: targetCid,
                      decoration: const InputDecoration(
                        labelText: '要撤回的 Manifest CID',
                      ),
                    ),
                  TextField(
                    controller: reason,
                    decoration: const InputDecoration(labelText: '说明（可选）'),
                  ),
                  if (validationError != null)
                    Text(
                      validationError!,
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(dialogContext, false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () {
                if (name.text.trim().isEmpty || passphrase.text.length < 10) {
                  setState(() => validationError = '请填写名称和至少 10 个字符的口令');
                  return;
                }
                if (operation == 'tombstone' && targetCid.text.trim().isEmpty) {
                  setState(() => validationError = '撤回操作必须填写目标 CID');
                  return;
                }
                try {
                  jsonDecode(bundle.text) as Map<String, dynamic>;
                  if (operation != 'tombstone') {
                    jsonDecode(manifest.text) as Map<String, dynamic>;
                  }
                } catch (error) {
                  setState(() => validationError = 'JSON 无效：$error');
                  return;
                }
                Navigator.pop(dialogContext, true);
              },
              child: const Text('签名并提交'),
            ),
          ],
        ),
      ),
    );
    if (accepted != true) return;
    await control.signPublication(
      displayName: name.text,
      passphrase: passphrase.text,
      bundle: jsonDecode(bundle.text) as Map<String, dynamic>,
      operation: operation,
      manifest: operation == 'tombstone'
          ? null
          : jsonDecode(manifest.text) as Map<String, dynamic>,
      targetCid: targetCid.text,
      reason: reason.text,
    );
    passphrase.clear();
    if (!context.mounted ||
        control.error != null ||
        control.lastResult == null) {
      return;
    }
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('发布完成'),
        content: SizedBox(
          width: 680,
          child: SingleChildScrollView(
            child: SelectableText(
              const JsonEncoder.withIndent('  ').convert(control.lastResult),
            ),
          ),
        ),
        actions: [
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text('完成'),
          ),
        ],
      ),
    );
  }
}

class _CommunityTab extends StatelessWidget {
  final ControlPlaneProvider control;
  const _CommunityTab(this.control);

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      floatingActionButton: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          FloatingActionButton.small(
            heroTag: 'follow-publisher',
            tooltip: '直接关注发布者（COM-003）',
            onPressed: () => _follow(context),
            child: const Icon(Icons.person_add_alt),
          ),
          const SizedBox(height: 10),
          FloatingActionButton.small(
            heroTag: 'policy-inspect',
            tooltip: '策略查询与本地覆盖（COM-011）',
            onPressed: () => _policyInspector(context),
            child: const Icon(Icons.policy),
          ),
          const SizedBox(height: 10),
          FloatingActionButton.small(
            heroTag: 'import-community-source',
            tooltip: '从 URI / CID / IPNS 导入（二维码使用同一 URI）',
            onPressed: () => _import(context),
            child: const Icon(Icons.qr_code_scanner),
          ),
          const SizedBox(height: 10),
          FloatingActionButton.extended(
            heroTag: 'add-community-source',
            onPressed: () => _add(context),
            icon: const Icon(Icons.add_link),
            label: const Text('添加社区源'),
          ),
        ],
      ),
      body:
          control.communitySources.isEmpty &&
              control.moderationReports.isEmpty &&
              control.follows.isEmpty &&
              control.refreshQueue.isEmpty
          ? const Center(child: Text('暂无社区源、关注或举报任务'))
          : ListView.builder(
              padding: const EdgeInsets.only(bottom: 88),
              itemCount:
                  _followsBase(control) +
                  control.refreshQueue.length +
                  control.communitySources.length +
                  control.moderationReports.length +
                  (control.moderationReports.isEmpty ? 0 : 1),
              itemBuilder: (context, index) {
                final followsBase = _followsBase(control);
                if (control.follows.isNotEmpty && index == 0) {
                  return const Padding(
                    padding: EdgeInsets.fromLTRB(16, 20, 16, 8),
                    child: Text(
                      '关注发布者',
                      style: TextStyle(fontWeight: FontWeight.bold),
                    ),
                  );
                }
                if (control.follows.isNotEmpty &&
                    index > 0 &&
                    index < followsBase) {
                  final follow = control.follows[index - 1];
                  final identityCid = '${follow['identity_cid'] ?? '-'}';
                  return Card(
                    margin: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 6,
                    ),
                    child: ListTile(
                      leading: const Icon(Icons.person),
                      title: Text('${follow['display_name'] ?? identityCid}'),
                      subtitle: Text(
                        '${follow['publisher_id'] ?? '-'}\n'
                        'Identity CID: $identityCid',
                      ),
                      trailing: IconButton(
                        tooltip: '取消关注',
                        onPressed: () => control.unfollowPublisher(identityCid),
                        icon: const Icon(Icons.person_remove),
                      ),
                    ),
                  );
                }
                if (index >= followsBase &&
                    index < followsBase + control.refreshQueue.length) {
                  final entry = control.refreshQueue[index - followsBase];
                  final queuedId = '${entry['source_id'] ?? '-'}';
                  return Card(
                    margin: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 6,
                    ),
                    child: ListTile(
                      leading: const Icon(Icons.cloud_off),
                      title: Text('离线刷新排队：$queuedId'),
                      subtitle: Text(
                        '已尝试 ${entry['attempts'] ?? 0} 次'
                        '${entry['last_error'] == null ? '' : ' · ${entry['last_error']}'}',
                      ),
                      trailing: IconButton(
                        tooltip: '立即重试',
                        onPressed: () => control.refreshCommunity(queuedId),
                        icon: const Icon(Icons.refresh),
                      ),
                    ),
                  );
                }
                final shifted =
                    index - followsBase - control.refreshQueue.length;
                if (shifted == control.communitySources.length &&
                    control.moderationReports.isNotEmpty) {
                  return const Padding(
                    padding: EdgeInsets.fromLTRB(16, 20, 16, 8),
                    child: Text(
                      '举报队列',
                      style: TextStyle(fontWeight: FontWeight.bold),
                    ),
                  );
                }
                if (shifted > control.communitySources.length ||
                    (control.communitySources.isEmpty &&
                        control.moderationReports.isNotEmpty)) {
                  final reportIndex =
                      shifted - control.communitySources.length - 1;
                  final record = control.moderationReports[reportIndex];
                  final report =
                      record['report'] as Map<String, dynamic>? ?? const {};
                  final reportId = '${report['report_id'] ?? '-'}';
                  final status = '${record['status'] ?? 'queued'}';
                  return Card(
                    margin: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 6,
                    ),
                    child: ListTile(
                      leading: Icon(
                        status == 'submitted'
                            ? Icons.verified
                            : status == 'failed'
                            ? Icons.error_outline
                            : Icons.schedule_send,
                      ),
                      title: Text('$reportId · $status'),
                      subtitle: Text(
                        '目标 ${report['target'] ?? '-'} · '
                        '接收方 ${report['recipient_source_id'] ?? '-'}\n'
                        '尝试 ${record['attempts'] ?? 0} 次'
                        '${record['next_retry_at'] == null ? '' : ' · 下次 ${record['next_retry_at']}'}'
                        '${record['last_error'] == null ? '' : ' · ${record['last_error']}'}',
                      ),
                      trailing: status == 'submitted'
                          ? null
                          : IconButton(
                              tooltip: '重试提交',
                              onPressed: () =>
                                  control.retryModerationReport(reportId),
                              icon: const Icon(Icons.refresh),
                            ),
                    ),
                  );
                }
                final source = control.communitySources[shifted];
                final manifest =
                    source['manifest'] as Map<String, dynamic>? ?? const {};
                final id = '${manifest['source_id'] ?? source['manifest_cid']}';
                final catalog = source['catalog_enabled'] == true;
                final policy = source['policy_enabled'] == true;
                return Card(
                  margin: const EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 6,
                  ),
                  child: ExpansionTile(
                    title: Text('${manifest['name'] ?? id}'),
                    subtitle: Text(
                      '$id · Catalog ${source['last_catalog_sequence'] ?? '-'}'
                      ' / Policy ${source['last_policy_sequence'] ?? '-'}'
                      '${source['bootstrap'] == true ? ' · 内置启动源' : ''}',
                    ),
                    children: [
                      ListTile(
                        title: const Text('维护者与签名'),
                        subtitle: SelectableText(
                          'Identity CID: ${manifest['maintainer_identity_cid'] ?? '-'}\n'
                          'Public key: ${source['maintainer_public_key'] ?? '-'}\n'
                          'Manifest CID: ${source['manifest_cid'] ?? '-'}\n'
                          'Trust order: ${source['trust_order'] ?? 0}\n'
                          'Key event: ${source['last_key_sequence'] ?? '-'} · '
                          '${source['maintainer_key_revoked'] == true ? '已撤销' : '有效'}',
                        ),
                      ),
                      if (source['last_error'] != null)
                        ListTile(
                          title: const Text('同步错误'),
                          subtitle: Text('${source['last_error']}'),
                        ),
                      SwitchListTile(
                        title: const Text('Catalog 索引'),
                        value: catalog,
                        onChanged: (value) => control.setCommunitySwitches(
                          id,
                          catalogEnabled: value,
                          policyEnabled: policy,
                        ),
                      ),
                      SwitchListTile(
                        title: const Text('Policy 规则'),
                        value: policy,
                        onChanged: (value) => control.setCommunitySwitches(
                          id,
                          catalogEnabled: catalog,
                          policyEnabled: value,
                        ),
                      ),
                      OverflowBar(
                        children: [
                          TextButton(
                            onPressed: () => control.refreshCommunity(id),
                            child: const Text('刷新'),
                          ),
                          TextButton(
                            onPressed: () => _keyEvent(context, id),
                            child: const Text('换钥/撤销'),
                          ),
                          TextButton(
                            onPressed: () => _report(context, id),
                            child: const Text('举报'),
                          ),
                          TextButton(
                            onPressed: () => control.removeCommunity(id),
                            child: const Text('移除'),
                          ),
                        ],
                      ),
                    ],
                  ),
                );
              },
            ),
    );
  }

  /// COM-011：策略查询 + 非强制策略的本地覆盖/取消。
  Future<void> _policyInspector(BuildContext context) async {
    final target = TextEditingController();
    final reason = TextEditingController();
    Map<String, dynamic>? decision;
    String? decisionError;
    final messenger = ScaffoldMessenger.of(context);
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: const Text('策略查询与本地覆盖'),
          content: SizedBox(
            width: 520,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                  controller: target,
                  decoration: const InputDecoration(
                    labelText: '目标 CID / 发布者 ID',
                  ),
                ),
                const SizedBox(height: 8),
                if (decision != null) ...[
                  SelectableText(
                    '动作：${decision!['action'] ?? '无'}\n'
                    '理由：${decision!['reason'] ?? '-'}\n'
                    '来源：${(decision!['source_ids'] as List<dynamic>? ?? const []).join(', ')}\n'
                    '到期：${decision!['expires_at'] ?? '无'}\n'
                    '本地覆盖：${decision!['locally_overridden'] ?? false}',
                  ),
                  const SizedBox(height: 8),
                  if (decision!['action'] != null &&
                      decision!['locally_overridden'] != true) ...[
                    TextField(
                      controller: reason,
                      decoration: const InputDecoration(
                        labelText: '覆盖理由（申诉说明）',
                      ),
                    ),
                    const SizedBox(height: 8),
                  ],
                  if (decision!['locally_overridden'] == true)
                    FilledButton(
                      onPressed: () async {
                        await control.clearPolicyOverride(target.text.trim());
                        messenger.showSnackBar(
                          const SnackBar(content: Text('已取消本地覆盖')),
                        );
                        if (dialogContext.mounted) {
                          Navigator.pop(dialogContext);
                        }
                      },
                      child: const Text('取消本地覆盖'),
                    ),
                ],
                if (decisionError != null)
                  Text(
                    decisionError!,
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(dialogContext),
              child: const Text('关闭'),
            ),
            FilledButton(
              onPressed: () async {
                final result = await control.policyDecision(target.text.trim());
                setState(() {
                  decision = result;
                  decisionError = result == null ? control.error : null;
                });
              },
              child: const Text('查询'),
            ),
            if (decision != null &&
                decision!['action'] != null &&
                decision!['locally_overridden'] != true)
              FilledButton(
                onPressed: () async {
                  await control.overridePolicy(
                    target.text.trim(),
                    reason.text.trim(),
                  );
                  messenger.showSnackBar(
                    const SnackBar(content: Text('已提交本地覆盖')),
                  );
                  final result = await control.policyDecision(
                    target.text.trim(),
                  );
                  setState(() => decision = result);
                },
                child: const Text('本地覆盖'),
              ),
          ],
        ),
      ),
    );
  }

  /// 直接关注发布者（COM-003）。
  Future<void> _follow(BuildContext context) async {
    final identityCid = TextEditingController();
    final publisherId = TextEditingController();
    final displayName = TextEditingController();
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('关注发布者'),
        content: SizedBox(
          width: 480,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: identityCid,
                decoration: const InputDecoration(
                  labelText: '发布者 Identity CID',
                ),
              ),
              TextField(
                controller: publisherId,
                decoration: const InputDecoration(labelText: 'publisher_id'),
              ),
              TextField(
                controller: displayName,
                decoration: const InputDecoration(labelText: '显示名称'),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext, true),
            child: const Text('关注'),
          ),
        ],
      ),
    );
    if (accepted != true) return;
    await control.followPublisher(
      identityCid.text,
      publisherId.text,
      displayName.text,
    );
  }

  Future<void> _add(BuildContext context) async {
    final publicKey = TextEditingController();
    final value = await _jsonDialog(
      context,
      '社区 Manifest JSON',
      const ['manifest'],
      extraController: publicKey,
      extraLabel: '维护者 Ed25519 公钥（hex）',
    );
    if (value != null) {
      await control.addCommunitySource(value['manifest']!, publicKey.text);
    }
  }

  Future<void> _import(BuildContext context) async {
    final locator = TextEditingController();
    final publicKey = TextEditingController();
    String? validationError;
    final result = await showDialog<MapEntry<String, String>>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: const Text('从 URI / CID / IPNS 导入'),
          content: SizedBox(
            width: 540,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                  controller: locator,
                  decoration: const InputDecoration(
                    labelText: 'jimmusic://、ipfs://、ipns:// 或裸 CID',
                  ),
                ),
                TextField(
                  controller: publicKey,
                  decoration: const InputDecoration(
                    labelText: '维护者 Ed25519 公钥（hex）',
                  ),
                ),
                if (validationError != null)
                  Text(
                    validationError!,
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(dialogContext),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () {
                if (locator.text.trim().isEmpty ||
                    publicKey.text.trim().isEmpty) {
                  setState(() => validationError = '定位符和维护者公钥均不能为空');
                  return;
                }
                Navigator.pop(
                  dialogContext,
                  MapEntry(locator.text.trim(), publicKey.text.trim()),
                );
              },
              child: const Text('导入'),
            ),
          ],
        ),
      ),
    );
    locator.dispose();
    publicKey.dispose();
    if (result != null) {
      await control.importCommunitySource(result.key, result.value);
    }
  }

  Future<void> _keyEvent(BuildContext context, String sourceId) async {
    final value = await _jsonDialog(context, '维护者换钥或撤销事件', const ['event']);
    if (value != null) {
      await control.applyMaintainerKeyEvent(sourceId, value['event']!);
    }
  }

  Future<void> _report(BuildContext context, String sourceId) async {
    final value = await _jsonDialog(context, '签名举报（接收方 $sourceId）', const [
      'report',
    ]);
    if (value != null) {
      final report = value['report']!;
      report['recipient_source_id'] = sourceId;
      Map<String, dynamic>? source;
      for (final candidate in control.communitySources) {
        final candidateManifest =
            candidate['manifest'] as Map<String, dynamic>?;
        if (candidateManifest?['source_id'] == sourceId) {
          source = candidate;
          break;
        }
      }
      final manifest = source?['manifest'] as Map<String, dynamic>?;
      final hasEncryptionKey =
          '${manifest?['report_encryption_public_key'] ?? ''}'.isNotEmpty;
      var encrypt = false;
      if (hasEncryptionKey) {
        if (!context.mounted) return;
        final choice = await showDialog<bool>(
          context: context,
          builder: (dialogContext) => AlertDialog(
            title: const Text('举报隐私'),
            content: const Text('该社区发布了举报加密公钥。加密后，远端只会收到密文和最小路由信息。'),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(dialogContext, false),
                child: const Text('不加密'),
              ),
              FilledButton(
                onPressed: () => Navigator.pop(dialogContext, true),
                child: const Text('加密提交'),
              ),
            ],
          ),
        );
        if (choice == null) return;
        encrypt = choice;
      }
      await control.queueModerationReport(report, encryptForRecipient: encrypt);
    }
  }
}

class _PluginsTab extends StatelessWidget {
  final ControlPlaneProvider control;
  const _PluginsTab(this.control);

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      floatingActionButton: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          // PLG-005：社区插件目录（远程目录浏览/安装入口）。
          FloatingActionButton.extended(
            heroTag: 'plugin-catalog-fab',
            onPressed: () => _catalog(context),
            icon: const Icon(Icons.storefront_outlined),
            label: const Text('插件目录'),
          ),
          const SizedBox(height: 12),
          FloatingActionButton.extended(
            heroTag: 'plugin-install-fab',
            onPressed: () => _install(context),
            icon: const Icon(Icons.extension),
            label: const Text('安装签名插件'),
          ),
        ],
      ),
      body: control.plugins.isEmpty && control.installJournal.isEmpty
          ? const Center(child: Text('暂无已安装插件'))
          : ListView.builder(
              padding: const EdgeInsets.only(bottom: 88),
              itemCount: control.plugins.length + control.installJournal.length,
              itemBuilder: (context, index) {
                if (index >= control.plugins.length) {
                  // PLG-013：安装中间态日志（下载/验证/暂存/提交/失败/中断）。
                  final entry =
                      control.installJournal[index - control.plugins.length];
                  final stage = '${entry['stage'] ?? 'unknown'}';
                  return Card(
                    margin: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 6,
                    ),
                    child: ListTile(
                      leading: Icon(
                        stage == 'failed' || stage == 'interrupted'
                            ? Icons.error_outline
                            : Icons.downloading,
                      ),
                      title: Text(
                        '安装日志 · ${entry['plugin_id'] ?? '-'} '
                        '${entry['version'] ?? ''}',
                      ),
                      subtitle: Text(
                        '$stage'
                        '${entry['error'] == null ? '' : ' · ${entry['error']}'}',
                      ),
                    ),
                  );
                }
                final plugin = control.plugins[index];
                final id = '${plugin['plugin_id']}';
                final state = '${plugin['lifecycle_state']}';
                final declared =
                    (plugin['permissions_declared'] as List<dynamic>? ??
                            const [])
                        .join(', ');
                final granted =
                    (plugin['permissions_granted'] as List<dynamic>? ??
                            const [])
                        .join(', ');
                final dependencies =
                    (plugin['dependencies'] as List<dynamic>? ?? const [])
                        .map((value) => jsonEncode(value))
                        .join('\n');
                final conflicts =
                    (plugin['conflicts'] as List<dynamic>? ?? const []).join(
                      ', ',
                    );
                return Card(
                  margin: const EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 6,
                  ),
                  child: ExpansionTile(
                    leading: const Icon(Icons.extension_outlined),
                    title: Text('${plugin['name'] ?? id}'),
                    subtitle: Text(
                      '$state · ${plugin['active_version'] ?? '-'} · ${plugin['trust_channel']}',
                    ),
                    children: [
                      // PLG-007：社区原生插件的持续警告。
                      if (plugin['trust_channel'] ==
                          'community_native_advanced')
                        const CommunityNativeWarningTile(),
                      ListTile(
                        title: const Text('权限'),
                        subtitle: Text(
                          '声明：${declared.isEmpty ? '-' : declared}\n'
                          '已授予：${granted.isEmpty ? '-' : granted}',
                        ),
                      ),
                      ListTile(
                        title: const Text('依赖 / 冲突'),
                        subtitle: Text(
                          '${dependencies.isEmpty ? '无依赖' : dependencies}\n'
                          '冲突：${conflicts.isEmpty ? '-' : conflicts}',
                        ),
                      ),
                      if (plugin['last_error'] != null)
                        ListTile(
                          title: const Text('最近错误'),
                          subtitle: Text('${plugin['last_error']}'),
                        ),
                      OverflowBar(
                        children: [
                          TextButton(
                            onPressed: () => control.pluginAction(id, 'enable'),
                            child: const Text('启用'),
                          ),
                          TextButton(
                            onPressed: () =>
                                control.pluginAction(id, 'disable'),
                            child: const Text('停用'),
                          ),
                          TextButton(
                            onPressed: () => _configure(context, plugin),
                            child: const Text('配置'),
                          ),
                          TextButton(
                            onPressed: () =>
                                control.pluginAction(id, 'rollback'),
                            child: const Text('回滚'),
                          ),
                          TextButton(
                            onPressed: () => control.uninstallPlugin(id),
                            child: const Text('卸载'),
                          ),
                        ],
                      ),
                    ],
                  ),
                );
              },
            ),
    );
  }

  Future<void> _install(BuildContext context) async {
    final publicKey = TextEditingController();
    final location = TextEditingController();
    final permissions = TextEditingController();
    final value = await _jsonDialog(
      context,
      'Plugin Manifest JSON',
      const ['manifest'],
      extraController: publicKey,
      extraLabel: '发布者 Ed25519 公钥（hex）',
      secondExtraController: location,
      secondExtraLabel: '制品 URL / ipfs://CID（可选）',
      thirdExtraController: permissions,
      thirdExtraLabel: '授予权限（逗号分隔）',
    );
    if (value != null) {
      await control.installPlugin(
        value['manifest']!,
        publicKey.text,
        artifactLocation: location.text,
        grantedPermissions: permissions.text
            .split(',')
            .map((value) => value.trim())
            .where((value) => value.isNotEmpty)
            .toList(),
      );
    }
  }

  /// PLG-005：浏览社区目录收录的插件清单。
  Future<void> _catalog(BuildContext context) async {
    final catalog = await control.pluginCatalog();
    if (!context.mounted) return;
    var entries = (catalog?['entries'] as List<dynamic>? ?? const [])
        .cast<Map<String, dynamic>>();
    final search = TextEditingController();
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: const Text('插件目录'),
          content: SizedBox(
            width: 640,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: TextField(
                        controller: search,
                        decoration: const InputDecoration(
                          labelText: '搜索 CID / 分类 / 标签 / 注解',
                        ),
                        onSubmitted: (_) => _searchCatalog(
                          dialogContext,
                          search,
                          setState,
                          (value) => entries = value,
                        ),
                      ),
                    ),
                    IconButton(
                      icon: const Icon(Icons.search),
                      tooltip: '搜索',
                      onPressed: () => _searchCatalog(
                        dialogContext,
                        search,
                        setState,
                        (value) => entries = value,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                Flexible(
                  child: entries.isEmpty
                      ? const SizedBox(
                          height: 120,
                          child: Center(child: Text('社区目录暂未收录插件清单')),
                        )
                      : ConstrainedBox(
                          constraints: const BoxConstraints(maxHeight: 420),
                          child: ListView.builder(
                            shrinkWrap: true,
                            itemCount: entries.length,
                            itemBuilder: (context, index) {
                              final entry = entries[index];
                              final cid = '${entry['target_cid']}';
                              final categories =
                                  (entry['categories'] as List<dynamic>? ??
                                          const [])
                                      .join(' · ');
                              final tags =
                                  (entry['tags'] as List<dynamic>? ?? const [])
                                      .join(' ');
                              return ListTile(
                                leading: const Icon(Icons.storefront_outlined),
                                title: Text(
                                  '${entry['annotation'] ?? cid}',
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                ),
                                subtitle: Text(
                                  '$cid${tags.isEmpty ? '' : '\n$tags'}',
                                  maxLines: 2,
                                  overflow: TextOverflow.ellipsis,
                                ),
                                trailing: categories.isEmpty
                                    ? null
                                    : Text(categories),
                                onTap: () {
                                  Navigator.pop(dialogContext);
                                  _catalogDetail(context, cid);
                                },
                              );
                            },
                          ),
                        ),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(dialogContext),
              child: const Text('关闭'),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _searchCatalog(
    BuildContext dialogContext,
    TextEditingController search,
    StateSetter setState,
    void Function(List<Map<String, dynamic>>) apply,
  ) async {
    final result = await control.pluginCatalog(q: search.text);
    if (!dialogContext.mounted) return;
    setState(
      () => apply(
        (result?['entries'] as List<dynamic>? ?? const [])
            .cast<Map<String, dynamic>>(),
      ),
    );
  }

  /// PLG-005：目录条目详情（Manifest 摘要 + 安装可行性），可从详情进入安装。
  Future<void> _catalogDetail(BuildContext context, String cid) async {
    final detail = await control.pluginCatalogDetail(cid);
    if (!context.mounted) return;
    if (detail == null) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('目录详情加载失败：${control.error ?? '未知错误'}')),
      );
      return;
    }
    final manifest = detail['manifest'] as Map<String, dynamic>? ?? const {};
    final name = '${manifest['name'] ?? '未知插件'}';
    final version = '${manifest['version'] ?? '-'}';
    final artifactAvailable = detail['artifact_available'] == true;
    final revoked = detail['revoked'] == true;
    final updateAvailable = detail['update_available'] == true;
    final activeVersion = detail['active_version'];
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text(name),
        content: SizedBox(
          width: 600,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '${manifest['plugin_id']} · v$version\n'
                  '发布者：${manifest['publisher'] ?? '-'} · '
                  '${manifest['plugin_kind'] ?? '-'} · '
                  '${manifest['license'] ?? '-'}',
                ),
                const SizedBox(height: 8),
                Text(
                  'CID：$cid\n'
                  '目标平台：${detail['platform']}/${detail['architecture']}',
                ),
                const SizedBox(height: 8),
                if (activeVersion != null)
                  Text(
                    '已安装版本：$activeVersion'
                    '（${detail['installed_state'] ?? '未知状态'}）',
                  ),
                if (updateAvailable) const Text('目录中存在可用的新版本。'),
                if (!artifactAvailable)
                  Text(
                    '当前平台/架构无可用制品，无法安装。',
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
                if (revoked)
                  Text(
                    '该版本已被目录策略撤销，无法安装。',
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
                const SizedBox(height: 8),
                if ((manifest['capabilities'] as List<dynamic>? ?? const [])
                    .isNotEmpty)
                  Text(
                    '能力：${(manifest['capabilities'] as List<dynamic>).join(', ')}',
                  ),
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text('关闭'),
          ),
          if (artifactAvailable && !revoked)
            FilledButton(
              onPressed: () {
                Navigator.pop(dialogContext);
                _installFromCatalog(context, detail);
              },
              child: Text(updateAvailable ? '更新' : '安装'),
            ),
        ],
      ),
    );
  }

  /// PLG-005：从目录条目安装——Manifest 已可信，只需发布者公钥与授权确认。
  Future<void> _installFromCatalog(
    BuildContext context,
    Map<String, dynamic> detail,
  ) async {
    final manifest = detail['manifest'] as Map<String, dynamic>? ?? const {};
    final cid = '${detail['manifest_cid']}';
    final publicKey = TextEditingController();
    final permissions = TextEditingController();
    var allowNative = false;
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: Text('从目录安装 ${manifest['name']}'),
          content: SizedBox(
            width: 600,
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    'manifest_cid：$cid',
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                  ),
                  const SizedBox(height: 12),
                  TextField(
                    controller: publicKey,
                    decoration: const InputDecoration(
                      labelText: '发布者 Ed25519 公钥（hex）',
                    ),
                  ),
                  TextField(
                    controller: permissions,
                    decoration: const InputDecoration(
                      labelText: '授予权限（逗号分隔，可留空）',
                    ),
                  ),
                  CheckboxListTile(
                    contentPadding: EdgeInsets.zero,
                    title: const Text('允许社区原生制品（在受限目录中运行）'),
                    value: allowNative,
                    onChanged: (value) =>
                        setState(() => allowNative = value ?? false),
                  ),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(dialogContext),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(dialogContext, true),
              child: const Text('安装'),
            ),
          ],
        ),
      ),
    );
    if (accepted == true) {
      // PLG-007：社区原生高级授权需要二次确认与持续警告。
      if (allowNative) {
        if (!context.mounted) return;
        final confirmed = await confirmCommunityNative(
          context,
          pluginName: '${manifest['name'] ?? '未知插件'}',
        );
        if (!confirmed) return;
      }
      await control.installPlugin(
        manifest,
        publicKey.text,
        artifactLocation: 'ipfs://$cid',
        grantedPermissions: permissions.text
            .split(',')
            .map((value) => value.trim())
            .where((value) => value.isNotEmpty)
            .toList(),
        allowCommunityNative: allowNative,
      );
    }
  }

  Future<void> _configure(
    BuildContext context,
    Map<String, dynamic> plugin,
  ) async {
    final id = '${plugin['plugin_id']}';
    final configuration = TextEditingController(
      text: const JsonEncoder.withIndent(
        '  ',
      ).convert(plugin['configuration'] ?? <String, dynamic>{}),
    );
    // PLG-014/UI-101：Schema 可解析时按声明式控件渲染，否则回退 JSON 编辑。
    final schema = await control.pluginConfigSchema(id);
    final current = await control.pluginConfig(id);
    final rawConfiguration = current?['configuration'];
    final initialConfiguration = rawConfiguration is Map<String, dynamic>
        ? rawConfiguration
        : <String, dynamic>{};
    if (!context.mounted) return;
    String? validationError;
    Map<String, dynamic>? schemaValues;
    final accepted = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setState) => AlertDialog(
          title: Text('配置 ${plugin['name'] ?? id}'),
          content: SizedBox(
            width: 600,
            child: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text('Schema CID：${plugin['configuration_schema_cid']}'),
                  const SizedBox(height: 8),
                  if (schema != null)
                    PluginConfigForm(
                      schema: schema,
                      initial: initialConfiguration,
                      onChanged: (values) => schemaValues = values,
                    )
                  else
                    TextField(
                      controller: configuration,
                      minLines: 8,
                      maxLines: 16,
                      decoration: const InputDecoration(
                        labelText: '声明式配置 JSON（Schema 不可解析）',
                        border: OutlineInputBorder(),
                      ),
                    ),
                  if (validationError != null)
                    Text(
                      validationError!,
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(dialogContext, false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () {
                if (schema == null) {
                  try {
                    jsonDecode(configuration.text) as Map<String, dynamic>;
                    Navigator.pop(dialogContext, true);
                  } catch (error) {
                    setState(() => validationError = '配置 JSON 无效：$error');
                  }
                } else {
                  Navigator.pop(dialogContext, true);
                }
              },
              child: const Text('验证并保存'),
            ),
          ],
        ),
      ),
    );
    if (accepted == true) {
      final values = schema == null
          ? jsonDecode(configuration.text) as Map<String, dynamic>
          : (schemaValues ?? initialConfiguration);
      await control.configurePlugin(id, values);
    }
  }
}

class _AudioPathTab extends StatelessWidget {
  final ControlPlaneProvider control;
  const _AudioPathTab(this.control);

  @override
  Widget build(BuildContext context) {
    final outputSession = context.watch<AudioOutputProvider>().session;
    final value = control.audioPath;
    if (value == null) {
      return ListView(
        padding: const EdgeInsets.all(16),
        children: [
          _OpenedOutputSession(session: outputSession),
          const SizedBox(height: 12),
          const Card(
            child: ListTile(
              leading: Icon(Icons.link_off),
              title: Text('控制服务未连接'),
              subtitle: Text('输出会话来自应用内 Rust Core；连接控制服务后可同时查看完整音频图与实时统计。'),
            ),
          ),
        ],
      );
    }
    final path = value['path'] as Map<String, dynamic>? ?? const {};
    final bitPerfect =
        value['bit_perfect'] as Map<String, dynamic>? ?? const {};
    final nodes = path['nodes'] as List<dynamic>? ?? const [];
    final conversions =
        path['format_conversions'] as List<dynamic>? ?? const [];
    final compensation =
        path['delay_compensation'] as List<dynamic>? ?? const [];
    final conditions = bitPerfect['conditions'] as List<dynamic>? ?? const [];
    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
        _StatusCard(
          title: 'Audio Path',
          status: '${path['mode'] ?? 'unknown'}',
          rows: {
            'Graph': '${path['graph_id']} @ ${path['graph_version']}',
            'Generation': '${path['generation']}',
            '总延迟': '${path['total_latency_frames']} frames',
            '预估缓冲': _bytes(path['estimated_buffer_bytes']),
            'Bit-perfect': '${bitPerfect['state'] ?? 'unknown'}',
            '边界说明': '${bitPerfect['statement'] ?? '-'}',
          },
        ),
        const SizedBox(height: 12),
        _OpenedOutputSession(session: outputSession),
        const SizedBox(height: 12),
        SwitchListTile.adaptive(
          value: path['mode'] == 'bit_perfect',
          onChanged: control.audioGraph == null
              ? null
              : (enabled) => control.setBitPerfectMode(enabled),
          title: const Text('Bit-perfect 模式'),
          subtitle: const Text('启用后禁止格式转换和普通 PCM 处理节点；条件不满足会明确失败，不会静默降级。'),
        ),
        if (conditions.isNotEmpty) ...[
          const SizedBox(height: 12),
          const Text(
            'Bit-perfect 条件',
            style: TextStyle(fontWeight: FontWeight.bold),
          ),
          ...conditions.map((raw) {
            final condition = raw as Map<String, dynamic>;
            final satisfied = condition['satisfied'] == true;
            return ListTile(
              leading: Icon(
                satisfied ? Icons.check_circle : Icons.cancel,
                color: satisfied
                    ? Colors.green
                    : Theme.of(context).colorScheme.error,
              ),
              title: Text('${condition['condition']}'),
              subtitle: condition['reason'] == null
                  ? null
                  : Text('${condition['reason']}'),
            );
          }),
        ],
        const SizedBox(height: 12),
        const Text('执行顺序', style: TextStyle(fontWeight: FontWeight.bold)),
        ...nodes.map((raw) {
          final node = raw as Map<String, dynamic>;
          return ListTile(
            leading: const Icon(Icons.memory),
            title: Text('${node['node_id']}'),
            subtitle: Text(
              '${node['node_type']} · ${node['accumulated_latency_frames']} frames',
            ),
          );
        }),
        const SizedBox(height: 12),
        ExpansionTile(
          title: Text('格式转换（${conversions.length}）'),
          subtitle: conversions.isEmpty ? const Text('无格式转换') : null,
          children: conversions
              .map(
                (raw) => Padding(
                  padding: const EdgeInsets.all(12),
                  child: SelectableText(
                    const JsonEncoder.withIndent('  ').convert(raw),
                  ),
                ),
              )
              .toList(),
        ),
        ExpansionTile(
          title: Text('延迟补偿（${compensation.length}）'),
          subtitle: compensation.isEmpty ? const Text('无需补偿') : null,
          children: compensation
              .map(
                (raw) => Padding(
                  padding: const EdgeInsets.all(12),
                  child: SelectableText(
                    const JsonEncoder.withIndent('  ').convert(raw),
                  ),
                ),
              )
              .toList(),
        ),
        const SizedBox(height: 12),
        ExpansionTile(
          title: const Text('实时统计'),
          children: [
            Padding(
              padding: const EdgeInsets.all(12),
              child: SelectableText(
                const JsonEncoder.withIndent(
                  '  ',
                ).convert(control.audioStats ?? {}),
              ),
            ),
          ],
        ),
      ],
    );
  }
}

class _OpenedOutputSession extends StatelessWidget {
  final Map<String, dynamic>? session;

  const _OpenedOutputSession({required this.session});

  @override
  Widget build(BuildContext context) {
    final value = session;
    if (value == null) {
      return const _StatusCard(
        title: '已打开输出会话',
        status: '未打开',
        rows: {'说明': '尚无驱动协商证据；不会据此声明 Bit-perfect 成功'},
      );
    }
    final format = value['negotiated_format'] as Map<String, dynamic>? ?? {};
    final exclusive = value['exclusive'] == true;
    return _StatusCard(
      title: '已打开输出会话',
      status: exclusive ? 'exclusive' : '${value['share_mode'] ?? 'unknown'}',
      rows: {
        '会话': '${value['session_id'] ?? '-'}',
        '设备': '${value['device_name'] ?? value['device_id'] ?? '-'}',
        '驱动': '${value['driver'] ?? '-'}',
        '协商格式':
            '${format['sample_rate'] ?? '-'} Hz / '
            '${format['channels'] ?? '-'} ch / '
            '${format['bit_depth'] ?? '-'} bit / '
            '${format['packing'] ?? '-'}',
        '缓冲':
            '软件 ${value['software_buffer_frames'] ?? '-'} 帧 / '
            '设备 ${value['device_buffer_frames'] ?? '驱动未暴露'}',
        '时钟': '${value['clock_source'] ?? '-'}',
        '证据来源': '${value['capability_source'] ?? '-'}',
      },
    );
  }
}

class _StatusCard extends StatelessWidget {
  final String title;
  final String status;
  final Map<String, String> rows;
  const _StatusCard({
    required this.title,
    required this.status,
    required this.rows,
  });

  @override
  Widget build(BuildContext context) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    title,
                    style: const TextStyle(
                      fontSize: 18,
                      fontWeight: FontWeight.bold,
                    ),
                  ),
                ),
                Chip(label: Text(status)),
              ],
            ),
            ...rows.entries.map(
              (entry) => Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    SizedBox(width: 120, child: Text(entry.key)),
                    Expanded(child: SelectableText(entry.value)),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ConnectHint extends StatelessWidget {
  const _ConnectHint();
  @override
  Widget build(BuildContext context) =>
      const Center(child: Text('请先配置并连接本地控制面'));
}

Future<Map<String, Map<String, dynamic>>?> _jsonDialog(
  BuildContext context,
  String title,
  List<String> fields, {
  TextEditingController? extraController,
  String? extraLabel,
  TextEditingController? secondExtraController,
  String? secondExtraLabel,
  TextEditingController? thirdExtraController,
  String? thirdExtraLabel,
}) async {
  final controllers = {
    for (final field in fields) field: TextEditingController(),
  };
  String? validationError;
  return showDialog<Map<String, Map<String, dynamic>>>(
    context: context,
    builder: (dialogContext) => StatefulBuilder(
      builder: (context, setState) => AlertDialog(
        title: Text(title),
        content: SizedBox(
          width: 600,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                ...controllers.entries.map(
                  (entry) => Padding(
                    padding: const EdgeInsets.only(bottom: 12),
                    child: TextField(
                      controller: entry.value,
                      minLines: 5,
                      maxLines: 12,
                      decoration: InputDecoration(
                        labelText: '${entry.key} JSON',
                        border: const OutlineInputBorder(),
                      ),
                    ),
                  ),
                ),
                if (extraController != null)
                  TextField(
                    controller: extraController,
                    decoration: InputDecoration(labelText: extraLabel),
                  ),
                if (secondExtraController != null)
                  TextField(
                    controller: secondExtraController,
                    decoration: InputDecoration(labelText: secondExtraLabel),
                  ),
                if (thirdExtraController != null)
                  TextField(
                    controller: thirdExtraController,
                    decoration: InputDecoration(labelText: thirdExtraLabel),
                  ),
                if (validationError != null)
                  Padding(
                    padding: const EdgeInsets.only(top: 8),
                    child: Text(
                      validationError!,
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () {
              try {
                final values = <String, Map<String, dynamic>>{};
                for (final entry in controllers.entries) {
                  values[entry.key] =
                      jsonDecode(entry.value.text) as Map<String, dynamic>;
                }
                Navigator.pop(dialogContext, values);
              } catch (error) {
                setState(() => validationError = 'JSON 无效：$error');
              }
            },
            child: const Text('提交'),
          ),
        ],
      ),
    ),
  );
}

String _bytes(dynamic value) {
  final bytes = (value as num?)?.toDouble() ?? 0;
  if (bytes >= 1024 * 1024 * 1024) {
    return '${(bytes / (1024 * 1024 * 1024)).toStringAsFixed(2)} GiB';
  }
  if (bytes >= 1024 * 1024) {
    return '${(bytes / (1024 * 1024)).toStringAsFixed(2)} MiB';
  }
  if (bytes >= 1024) return '${(bytes / 1024).toStringAsFixed(2)} KiB';
  return '${bytes.toInt()} B';
}

/// 从保存路径判断是否是可边下边播的音频文件，返回对应 MIME。
int _followsBase(ControlPlaneProvider control) =>
    control.follows.isEmpty ? 0 : control.follows.length + 1;

String? _audioMimeForPath(String path) {
  final lower = path.toLowerCase();
  if (lower.endsWith('.mp3')) return 'audio/mpeg';
  if (lower.endsWith('.m4a')) return 'audio/mp4';
  if (lower.endsWith('.aac')) return 'audio/aac';
  if (lower.endsWith('.flac')) return 'audio/flac';
  if (lower.endsWith('.wav')) return 'audio/wav';
  if (lower.endsWith('.ogg') || lower.endsWith('.opus')) return 'audio/ogg';
  return null;
}
