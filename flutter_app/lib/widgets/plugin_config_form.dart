import 'package:flutter/material.dart';

/// 从声明式 Schema 计算默认配置（PLG-014/UI-101）。
/// 支持 JSON Schema 子集：boolean/integer/number/string 与 enum。
Map<String, dynamic> schemaDefaults(Map<String, dynamic> schema) {
  final properties =
      schema['properties'] is Map<String, dynamic>
          ? schema['properties'] as Map<String, dynamic>
          : const <String, dynamic>{};
  final result = <String, dynamic>{};
  for (final entry in properties.entries) {
    final property = entry.value is Map<String, dynamic>
        ? entry.value as Map<String, dynamic>
        : const <String, dynamic>{};
    if (property.containsKey('default')) {
      result[entry.key] = property['default'];
      continue;
    }
    final enums = property['enum'];
    if (enums is List && enums.isNotEmpty) {
      result[entry.key] = enums.first;
      continue;
    }
    switch (property['type']) {
      case 'boolean':
        result[entry.key] = false;
      case 'integer':
      case 'number':
        result[entry.key] =
            property['minimum'] is num ? property['minimum'] : 0;
      case 'string':
        result[entry.key] = '';
      default:
        result[entry.key] = null;
    }
  }
  return result;
}

/// 按声明式配置 Schema 渲染 Host 组件（开关/枚举下拉/数值滑杆/文本框）。
/// 只使用 Flutter 内置组件与参数绑定，不执行插件注入的任意 UI 代码。
class PluginConfigForm extends StatefulWidget {
  const PluginConfigForm({
    super.key,
    required this.schema,
    required this.initial,
    required this.onChanged,
  });

  final Map<String, dynamic> schema;
  final Map<String, dynamic> initial;
  final ValueChanged<Map<String, dynamic>> onChanged;

  @override
  State<PluginConfigForm> createState() => _PluginConfigFormState();
}

class _PluginConfigFormState extends State<PluginConfigForm> {
  late final Map<String, dynamic> _values = {
    ...schemaDefaults(widget.schema),
    ...widget.initial,
  };

  void _update(String key, dynamic value) {
    setState(() => _values[key] = value);
    widget.onChanged(Map<String, dynamic>.from(_values));
  }

  @override
  Widget build(BuildContext context) {
    final properties =
        widget.schema['properties'] is Map<String, dynamic>
            ? widget.schema['properties'] as Map<String, dynamic>
            : const <String, dynamic>{};
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final entry in properties.entries)
          _field(
            entry.key,
            entry.value is Map<String, dynamic>
                ? entry.value as Map<String, dynamic>
                : const <String, dynamic>{},
          ),
      ],
    );
  }

  Widget _field(String key, Map<String, dynamic> property) {
    final title = '${property['title'] ?? key}';
    final description = property['description'];
    final enums = property['enum'];
    if (enums is List && enums.isNotEmpty) {
      return DropdownButtonFormField<Object>(
        initialValue: _values[key],
        decoration: InputDecoration(
          labelText: title,
          helperText: description == null ? null : '$description',
        ),
        items: [
          for (final option in enums)
            DropdownMenuItem(value: option, child: Text('$option')),
        ],
        onChanged: (value) {
          if (value != null) _update(key, value);
        },
      );
    }
    switch (property['type']) {
      case 'boolean':
        return SwitchListTile(
          title: Text(title),
          subtitle: description == null ? null : Text('$description'),
          value: _values[key] == true,
          onChanged: (value) => _update(key, value),
        );
      case 'integer':
      case 'number':
        final minimum = property['minimum'] is num
            ? (property['minimum'] as num).toDouble()
            : null;
        final maximum = property['maximum'] is num
            ? (property['maximum'] as num).toDouble()
            : null;
        if (minimum != null && maximum != null) {
          final current =
              (_values[key] as num?)?.toDouble() ?? minimum;
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              ListTile(
                dense: true,
                title: Text('$title：${_values[key] ?? minimum}'),
                subtitle: description == null ? null : Text('$description'),
              ),
              Slider(
                value: current.clamp(minimum, maximum),
                min: minimum,
                max: maximum,
                onChanged: (value) {
                  final resolved =
                      property['type'] == 'integer' ? value.round() : value;
                  _update(key, resolved);
                },
              ),
            ],
          );
        }
        return TextFormField(
          initialValue: '${_values[key] ?? ''}',
          keyboardType: const TextInputType.numberWithOptions(
            decimal: true,
            signed: true,
          ),
          decoration: InputDecoration(
            labelText: title,
            helperText: description == null ? null : '$description',
          ),
          onChanged: (text) {
            final parsed = int.tryParse(text) ?? double.tryParse(text);
            if (parsed != null) _update(key, parsed);
          },
        );
      case 'string':
      default:
        // PLG-011：敏感字段（如口令/令牌）默认遮罩显示。
        final sensitive = property['sensitive'] == true;
        return TextFormField(
          initialValue: '${_values[key] ?? ''}',
          obscureText: sensitive,
          decoration: InputDecoration(
            labelText: sensitive ? '$title（敏感）' : title,
            helperText: description == null ? null : '$description',
          ),
          onChanged: (text) => _update(key, text),
        );
    }
  }
}
