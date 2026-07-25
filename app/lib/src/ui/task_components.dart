import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:intl/intl.dart' hide TextDirection;
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/core/task_due.dart';
import 'package:taskveil/src/core/task_tree.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/ui/theme.dart';

part 'task_components/task_tokens.dart';
part 'task_components/task_capture.dart';
part 'task_components/task_metadata.dart';
part 'task_components/task_rows.dart';
part 'task_components/task_completion.dart';
part 'task_components/task_priority_dot.dart';
