import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'screens/home_screen.dart';
import 'providers/audio_output_provider.dart';
import 'providers/music_player_provider.dart';
import 'providers/theme_provider.dart';
import 'providers/control_plane_provider.dart';

void main() {
  runApp(const JimMusicApp());
}

class JimMusicApp extends StatelessWidget {
  const JimMusicApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MultiProvider(
      providers: [
        ChangeNotifierProvider(create: (_) => MusicPlayerProvider()),
        ChangeNotifierProvider(create: (_) => ThemeProvider()..load()),
        ChangeNotifierProvider(create: (_) => AudioOutputProvider()..load()),
        ChangeNotifierProvider(create: (_) => ControlPlaneProvider()..load()),
      ],
      child: Consumer<ThemeProvider>(
        builder: (context, themeProvider, child) {
          return MaterialApp(
            title: 'JimMusic',
            theme: themeProvider.lightTheme,
            darkTheme: themeProvider.darkTheme,
            themeMode: themeProvider.themeMode,
            home: const HomeScreen(),
            debugShowCheckedModeBanner: false,
          );
        },
      ),
    );
  }
}
