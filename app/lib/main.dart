import 'dart:async';
import 'dart:io';
import 'dart:ui' show Locale, PlatformDispatcher;

import 'package:flutter/material.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_riverpod/misc.dart' show Override;
import 'package:go_router/go_router.dart';
import 'package:path_provider/path_provider.dart';
import 'package:taskveil/src/core/bridge_service.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/notifications/reminder_notifications.dart';
import 'package:taskveil/src/router.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/rust/frb_generated.dart';
import 'package:taskveil/src/screens/onboarding_screen.dart';
import 'package:taskveil/src/timer/timer_notifications.dart';
import 'package:taskveil/src/ui/theme.dart';
import 'package:taskveil/src/ui/bridge_error_messages.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();

  Object? initializationError;
  ReminderNotificationService? reminderNotificationService;
  TimerNotificationService? timerNotificationService;
  try {
    await RustLib.init();
    final supportDir = await getApplicationSupportDirectory();
    final dbDir = Directory('${supportDir.path}/taskveil-db');
    await dbDir.create(recursive: true);
    final defaultInboxName = lookupAppLocalizations(
      _resolveStartupLocale(PlatformDispatcher.instance.locale),
    ).defaultInboxName;
    await initCore(dbDir: dbDir.path, defaultInboxName: defaultInboxName);
    final startupL10n = lookupAppLocalizations(
      _resolveStartupLocale(PlatformDispatcher.instance.locale),
    );
    final notificationContent = ReminderNotificationContent(
      title: startupL10n.reminderNotificationTitle,
      body: startupL10n.reminderNotificationBody,
      snoozeActionTitle: startupL10n.reminderSnoozeOneHourAction,
    );
    final localNotificationsPlugin = FlutterLocalNotificationsPlugin();
    final notificationService = ReminderNotificationService(
      reminderBridge: const FrbBridgeService(),
      gateway: FlutterLocalReminderNotificationGateway(
        plugin: localNotificationsPlugin,
      ),
    );
    try {
      await notificationService.initialize(notificationContent);
      reminderNotificationService = notificationService;
    } catch (error) {
      debugPrint(
        'Taskveil reminder notification initialization failed: $error',
      );
    }
    timerNotificationService = TimerNotificationService(
      FlutterLocalTimerNotificationGateway(plugin: localNotificationsPlugin),
    );
    try {
      await timerNotificationService.initialize(
        TimerNotificationContent(
          title: startupL10n.timerNotificationTitle,
          body: startupL10n.timerNotificationBody,
        ),
      );
    } catch (error) {
      debugPrint('Taskveil timer notification initialization failed: $error');
    }
  } catch (error, stackTrace) {
    initializationError = error;
    debugPrint(
      'Taskveil native core initialization failed: $error\n$stackTrace',
    );
  }

  runApp(
    TaskveilApp(
      initializationError: initializationError,
      overrides: [
        if (reminderNotificationService != null)
          reminderNotificationServiceProvider.overrideWith((ref) {
            ref.onDispose(reminderNotificationService!.dispose);
            return reminderNotificationService;
          }),
        if (timerNotificationService != null)
          timerNotificationServiceProvider.overrideWithValue(
            timerNotificationService,
          ),
      ],
    ),
  );
  if (reminderNotificationService != null) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      reminderNotificationService!.requestReconciliation(rebuild: true);
    });
  }
}

Locale _resolveStartupLocale(Locale platformLocale) {
  for (final supportedLocale in AppLocalizations.supportedLocales) {
    if (supportedLocale.languageCode == platformLocale.languageCode) {
      return supportedLocale;
    }
  }
  return AppLocalizations.supportedLocales.first;
}

/// Top-level app widget.
///
/// This is intentionally separate from the native-initialization logic in
/// [main] so widget tests can build it directly -- with a fake
/// `BridgeService` supplied via [overrides] -- without ever calling
/// `RustLib.init()`, touching the filesystem, or calling `initCore`.
class TaskveilApp extends StatelessWidget {
  TaskveilApp({
    super.key,
    this.initializationError,
    this.overrides = const [],
    GoRouter? router,
  }) : router = router ?? buildAppRouter();

  /// If non-null, [main] failed to initialize the native core; a minimal
  /// error screen is shown instead of the app so the failure is visible
  /// rather than silently unusable.
  final Object? initializationError;

  /// Riverpod provider overrides. Widget tests use this to replace
  /// [bridgeServiceProvider] with an in-memory fake.
  final List<Override> overrides;

  /// The go_router instance backing this app. Defaults to a freshly built
  /// router (see `src/router.dart`); tests may supply their own if needed.
  final GoRouter router;

  @override
  Widget build(BuildContext context) {
    final error = initializationError;
    if (error != null) {
      return MaterialApp(
        localizationsDelegates: AppLocalizations.localizationsDelegates,
        supportedLocales: AppLocalizations.supportedLocales,
        onGenerateTitle: (context) => AppLocalizations.of(context)!.appTitle,
        theme: buildTaskveilTheme(Brightness.light),
        darkTheme: buildTaskveilTheme(Brightness.dark),
        home: Scaffold(
          body: Center(
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Builder(
                builder: (context) => Text(
                  AppLocalizations.of(context)!.failedToStartTaskveil(
                    bridgeErrorMessage(AppLocalizations.of(context)!, error),
                  ),
                ),
              ),
            ),
          ),
        ),
      );
    }

    return ProviderScope(
      overrides: overrides,
      child: _TaskveilAppShell(router: router),
    );
  }
}

class _TaskveilAppShell extends ConsumerStatefulWidget {
  const _TaskveilAppShell({required this.router});

  final GoRouter router;

  @override
  ConsumerState<_TaskveilAppShell> createState() => _TaskveilAppShellState();
}

class _TaskveilAppShellState extends ConsumerState<_TaskveilAppShell>
    with WidgetsBindingObserver {
  bool _startupSettlementRequested = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    final onboardingCompleted = ref.read(onboardingStatusProvider).value;
    if (state == AppLifecycleState.resumed) {
      ref.read(reminderNotificationServiceProvider).setForeground(true);
    } else if (state == AppLifecycleState.inactive ||
        state == AppLifecycleState.hidden ||
        state == AppLifecycleState.paused ||
        state == AppLifecycleState.detached) {
      ref.read(reminderNotificationServiceProvider).setForeground(false);
    }
    if (state == AppLifecycleState.resumed && onboardingCompleted == true) {
      ref.read(appForegroundProvider.notifier).setForeground(true);
      unawaited(_settleRecurrenceAndSync());
      unawaited(_settleTimerOnResume());
    } else if (state == AppLifecycleState.inactive ||
        state == AppLifecycleState.hidden ||
        state == AppLifecycleState.paused ||
        state == AppLifecycleState.detached) {
      ref.read(appForegroundProvider.notifier).setForeground(false);
      ref.read(syncStatusProvider.notifier).setForeground(false);
    }
  }

  Future<void> _settleTimerOnResume() async {
    try {
      await ref.read(timerEngineProvider.future);
      await ref.read(timerEngineProvider.notifier).settleOnResume();
    } catch (_) {
      // Resume settlement is retried on the next foreground/restart. Never
      // surface session details or turn a recoverable timer failure into an
      // unhandled lifecycle exception.
      ref.invalidate(timerEngineProvider);
    }
  }

  Future<void> _settleRecurrenceAndSync() async {
    try {
      var hasMore = false;
      do {
        final summary = await ref
            .read(bridgeServiceProvider)
            .settleDueSeries(atMs: DateTime.now().millisecondsSinceEpoch);
        hasMore = summary.hasMore;
        if (hasMore) {
          await Future<void>.delayed(Duration.zero);
        }
      } while (hasMore);
      ref.invalidate(listsProvider);
      ref.invalidate(tasksProvider);
      ref.invalidate(homeTasksProvider);
      ref.invalidate(calendarOccurrencesProvider);
    } catch (_) {
      // Each batch is transactional and is retried on the next lifecycle or
      // sync event.
    }
    await ref.read(syncStatusProvider.notifier).syncOnResume();
  }

  @override
  Widget build(BuildContext context) {
    final onboardingStatus = ref.watch(onboardingStatusProvider);
    return onboardingStatus.when(
      loading: () =>
          _buildOnboardingMaterialApp(home: const _OnboardingLoadingScreen()),
      error: (error, stackTrace) => _buildOnboardingMaterialApp(
        home: _OnboardingLoadErrorScreen(
          onRetry: () => ref.invalidate(onboardingStatusProvider),
        ),
      ),
      data: (completed) {
        if (!completed) {
          return _buildOnboardingMaterialApp(
            home: OnboardingScreen(
              onComplete: () =>
                  ref.read(onboardingStatusProvider.notifier).complete(),
            ),
          );
        }
        ref.watch(syncStatusProvider);
        ref.watch(realtimeConnectionControllerProvider);
        ref.watch(timerEngineProvider);
        if (!_startupSettlementRequested) {
          _startupSettlementRequested = true;
          scheduleMicrotask(_settleRecurrenceAndSync);
        }
        return MaterialApp.router(
          localizationsDelegates: AppLocalizations.localizationsDelegates,
          supportedLocales: AppLocalizations.supportedLocales,
          onGenerateTitle: (context) => AppLocalizations.of(context)!.appTitle,
          theme: buildTaskveilTheme(Brightness.light),
          darkTheme: buildTaskveilTheme(Brightness.dark),
          routerConfig: widget.router,
        );
      },
    );
  }

  MaterialApp _buildOnboardingMaterialApp({required Widget home}) {
    return MaterialApp(
      localizationsDelegates: AppLocalizations.localizationsDelegates,
      supportedLocales: AppLocalizations.supportedLocales,
      onGenerateTitle: (context) => AppLocalizations.of(context)!.appTitle,
      theme: buildTaskveilTheme(Brightness.light),
      darkTheme: buildTaskveilTheme(Brightness.dark),
      home: home,
    );
  }
}

class _OnboardingLoadingScreen extends StatelessWidget {
  const _OnboardingLoadingScreen();

  @override
  Widget build(BuildContext context) {
    return const Scaffold(body: Center(child: CircularProgressIndicator()));
  }
}

class _OnboardingLoadErrorScreen extends StatelessWidget {
  const _OnboardingLoadErrorScreen({required this.onRetry});

  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final l10n = AppLocalizations.of(context)!;
    return Scaffold(
      body: SafeArea(
        child: Center(
          child: Padding(
            padding: const EdgeInsets.all(AppSpacing.lg),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  l10n.onboardingLoadFailed,
                  textAlign: TextAlign.center,
                  style: Theme.of(context).textTheme.bodyLarge,
                ),
                const SizedBox(height: AppSpacing.md),
                FilledButton(onPressed: onRetry, child: Text(l10n.retryButton)),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
