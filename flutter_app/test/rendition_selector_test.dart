import 'package:flutter_app/services/rendition_selector.dart';
import 'package:flutter_test/flutter_test.dart';

Map<String, dynamic> source({
  required String cid,
  String container = 'flac',
  String codec = 'flac',
  bool lossless = true,
  bool original = false,
  bool streamable = true,
  int byteLength = 1024,
  String availability = 'available',
}) => {
  'source_id': 'src-$cid',
  'kind': 'ipfs',
  'uri': 'ipfs://$cid',
  'content_cid': cid,
  'container': container,
  'codec': codec,
  'lossless': lossless,
  'original': original,
  'streamable': streamable,
  'byte_length': byteLength,
  'availability': availability,
};

void main() {
  group('selectBestRenditionCid（DST-002）', () {
    test('空列表返回 null', () {
      expect(selectBestRenditionCid(const [], isWeb: false), isNull);
    });

    test('原生端非计量网络优先 lossless original', () {
      final cid = selectBestRenditionCid([
        source(
          cid: 'bafy-mp3',
          container: 'mp3',
          codec: 'mp3',
          lossless: false,
        ),
        source(cid: 'bafy-flac', original: true, lossless: true),
      ], isWeb: false);
      expect(cid, 'bafy-flac');
    });

    test('Web 端剔除浏览器无法解码的源', () {
      final cid = selectBestRenditionCid([
        source(
          cid: 'bafy-dsd',
          container: 'dsf',
          codec: 'dsd',
          lossless: true,
          original: true,
        ),
        source(
          cid: 'bafy-mp3',
          container: 'mp3',
          codec: 'mp3',
          lossless: false,
        ),
      ], isWeb: true);
      expect(cid, 'bafy-mp3');
    });

    test('计量网络偏好有损流式小体积', () {
      final cid = selectBestRenditionCid(
        [
          source(cid: 'bafy-flac', lossless: true, original: true),
          source(
            cid: 'bafy-aac',
            container: 'm4a',
            codec: 'aac',
            lossless: false,
            byteLength: 512,
          ),
        ],
        isWeb: false,
        preferCompact: true,
      );
      expect(cid, 'bafy-aac');
    });

    test('计量网络下同为有损选体积更小者', () {
      final cid = selectBestRenditionCid(
        [
          source(
            cid: 'bafy-big',
            container: 'mp3',
            codec: 'mp3',
            lossless: false,
            byteLength: 5 * 1024 * 1024,
          ),
          source(
            cid: 'bafy-small',
            container: 'mp3',
            codec: 'mp3',
            lossless: false,
            byteLength: 512,
          ),
        ],
        isWeb: true,
        preferCompact: true,
      );
      expect(cid, 'bafy-small');
    });

    test('全部不可用时回退任一可用 CID 候选', () {
      final cid = selectBestRenditionCid([
        source(cid: 'bafy-offline', availability: 'offline'),
        source(cid: 'bafy-missing', availability: 'missing'),
      ], isWeb: false);
      expect(cid, isNotNull);
    });

    test('无 CID 的源被忽略', () {
      final cid = selectBestRenditionCid([
        {'kind': 'ipfs', 'content_cid': null},
        source(cid: 'bafy-real'),
      ], isWeb: false);
      expect(cid, 'bafy-real');
    });
  });
}
