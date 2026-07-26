import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:intl/intl.dart' as intl;

import 'app_localizations_en.dart';
import 'app_localizations_ja.dart';

// ignore_for_file: type=lint

/// Callers can lookup localized strings with an instance of AppLocalizations
/// returned by `AppLocalizations.of(context)`.
///
/// Applications need to include `AppLocalizations.delegate()` in their app's
/// `localizationDelegates` list, and the locales they support in the app's
/// `supportedLocales` list. For example:
///
/// ```dart
/// import 'l10n/app_localizations.dart';
///
/// return MaterialApp(
///   localizationsDelegates: AppLocalizations.localizationsDelegates,
///   supportedLocales: AppLocalizations.supportedLocales,
///   home: MyApplicationHome(),
/// );
/// ```
///
/// ## Update pubspec.yaml
///
/// Please make sure to update your pubspec.yaml to include the following
/// packages:
///
/// ```yaml
/// dependencies:
///   # Internationalization support.
///   flutter_localizations:
///     sdk: flutter
///   intl: any # Use the pinned version from flutter_localizations
///
///   # Rest of dependencies
/// ```
///
/// ## iOS Applications
///
/// iOS applications define key application metadata, including supported
/// locales, in an Info.plist file that is built into the application bundle.
/// To configure the locales supported by your app, you’ll need to edit this
/// file.
///
/// First, open your project’s ios/Runner.xcworkspace Xcode workspace file.
/// Then, in the Project Navigator, open the Info.plist file under the Runner
/// project’s Runner folder.
///
/// Next, select the Information Property List item, select Add Item from the
/// Editor menu, then select Localizations from the pop-up menu.
///
/// Select and expand the newly-created Localizations item then, for each
/// locale your application supports, add a new item and select the locale
/// you wish to add from the pop-up menu in the Value field. This list should
/// be consistent with the languages listed in the AppLocalizations.supportedLocales
/// property.
abstract class AppLocalizations {
  AppLocalizations(String locale)
    : localeName = intl.Intl.canonicalizedLocale(locale.toString());

  final String localeName;

  static AppLocalizations? of(BuildContext context) {
    return Localizations.of<AppLocalizations>(context, AppLocalizations);
  }

  static const LocalizationsDelegate<AppLocalizations> delegate =
      _AppLocalizationsDelegate();

  /// A list of this localizations delegate along with the default localizations
  /// delegates.
  ///
  /// Returns a list of localizations delegates containing this delegate along with
  /// GlobalMaterialLocalizations.delegate, GlobalCupertinoLocalizations.delegate,
  /// and GlobalWidgetsLocalizations.delegate.
  ///
  /// Additional delegates can be added by appending to this list in
  /// MaterialApp. This list does not have to be used at all if a custom list
  /// of delegates is preferred or required.
  static const List<LocalizationsDelegate<dynamic>> localizationsDelegates =
      <LocalizationsDelegate<dynamic>>[
        delegate,
        GlobalMaterialLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
      ];

  /// A list of this localizations delegate's supported locales.
  static const List<Locale> supportedLocales = <Locale>[
    Locale('en'),
    Locale('ja'),
  ];

  /// No description provided for @appTitle.
  ///
  /// In en, this message translates to:
  /// **'Taskveil'**
  String get appTitle;

  /// No description provided for @defaultInboxName.
  ///
  /// In en, this message translates to:
  /// **'Inbox'**
  String get defaultInboxName;

  /// No description provided for @defaultListMissing.
  ///
  /// In en, this message translates to:
  /// **'Default list is missing. Restart Taskveil or check local database provisioning.'**
  String get defaultListMissing;

  /// No description provided for @listsTitle.
  ///
  /// In en, this message translates to:
  /// **'Lists'**
  String get listsTitle;

  /// No description provided for @listsSectionTitle.
  ///
  /// In en, this message translates to:
  /// **'Lists'**
  String get listsSectionTitle;

  /// No description provided for @listsEmpty.
  ///
  /// In en, this message translates to:
  /// **'No lists yet. Tap + to create one.'**
  String get listsEmpty;

  /// No description provided for @listsEmptyTitle.
  ///
  /// In en, this message translates to:
  /// **'No lists yet.'**
  String get listsEmptyTitle;

  /// No description provided for @listsEmptyBody.
  ///
  /// In en, this message translates to:
  /// **'Tap + to create one.'**
  String get listsEmptyBody;

  /// No description provided for @failedToLoadLists.
  ///
  /// In en, this message translates to:
  /// **'Failed to load lists: {error}'**
  String failedToLoadLists(String error);

  /// No description provided for @newListTooltip.
  ///
  /// In en, this message translates to:
  /// **'New list'**
  String get newListTooltip;

  /// No description provided for @newListTitle.
  ///
  /// In en, this message translates to:
  /// **'New list'**
  String get newListTitle;

  /// No description provided for @listActionsTooltip.
  ///
  /// In en, this message translates to:
  /// **'List actions'**
  String get listActionsTooltip;

  /// No description provided for @listsMoreMenuTooltip.
  ///
  /// In en, this message translates to:
  /// **'More'**
  String get listsMoreMenuTooltip;

  /// No description provided for @renameListMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Rename'**
  String get renameListMenuItem;

  /// No description provided for @renameListTitle.
  ///
  /// In en, this message translates to:
  /// **'Rename list'**
  String get renameListTitle;

  /// No description provided for @archiveListMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Archive'**
  String get archiveListMenuItem;

  /// No description provided for @deleteListMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Delete'**
  String get deleteListMenuItem;

  /// No description provided for @unarchiveListMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Unarchive'**
  String get unarchiveListMenuItem;

  /// No description provided for @deleteListDialogTitle.
  ///
  /// In en, this message translates to:
  /// **'Delete {listName}?'**
  String deleteListDialogTitle(String listName);

  /// No description provided for @deleteListDialogMessage.
  ///
  /// In en, this message translates to:
  /// **'This will permanently delete this list. Its {taskCount} tasks, including completed tasks, will move to Inbox and keep their reminders and history. Deleting the list cannot be undone.'**
  String deleteListDialogMessage(int taskCount);

  /// No description provided for @archivedListsSectionTitle.
  ///
  /// In en, this message translates to:
  /// **'Archived ({count})'**
  String archivedListsSectionTitle(int count);

  /// No description provided for @showArchivedListsTooltip.
  ///
  /// In en, this message translates to:
  /// **'Show archived lists'**
  String get showArchivedListsTooltip;

  /// No description provided for @hideArchivedListsTooltip.
  ///
  /// In en, this message translates to:
  /// **'Hide archived lists'**
  String get hideArchivedListsTooltip;

  /// No description provided for @nameLabel.
  ///
  /// In en, this message translates to:
  /// **'Name'**
  String get nameLabel;

  /// No description provided for @cancelButton.
  ///
  /// In en, this message translates to:
  /// **'Cancel'**
  String get cancelButton;

  /// No description provided for @deleteButton.
  ///
  /// In en, this message translates to:
  /// **'Delete'**
  String get deleteButton;

  /// No description provided for @createButton.
  ///
  /// In en, this message translates to:
  /// **'Create'**
  String get createButton;

  /// No description provided for @tasksTitle.
  ///
  /// In en, this message translates to:
  /// **'Tasks'**
  String get tasksTitle;

  /// No description provided for @homeTitle.
  ///
  /// In en, this message translates to:
  /// **'Home'**
  String get homeTitle;

  /// No description provided for @calendarTitle.
  ///
  /// In en, this message translates to:
  /// **'Calendar'**
  String get calendarTitle;

  /// No description provided for @calendarWeekTab.
  ///
  /// In en, this message translates to:
  /// **'Week'**
  String get calendarWeekTab;

  /// No description provided for @calendarMonthTab.
  ///
  /// In en, this message translates to:
  /// **'Month'**
  String get calendarMonthTab;

  /// No description provided for @calendarPreviousPeriodTooltip.
  ///
  /// In en, this message translates to:
  /// **'Previous period'**
  String get calendarPreviousPeriodTooltip;

  /// No description provided for @calendarNextPeriodTooltip.
  ///
  /// In en, this message translates to:
  /// **'Next period'**
  String get calendarNextPeriodTooltip;

  /// No description provided for @calendarGoToToday.
  ///
  /// In en, this message translates to:
  /// **'Today'**
  String get calendarGoToToday;

  /// No description provided for @calendarSelectedDaySemantics.
  ///
  /// In en, this message translates to:
  /// **'Selected date: {date}'**
  String calendarSelectedDaySemantics(String date);

  /// No description provided for @calendarDayTaskCount.
  ///
  /// In en, this message translates to:
  /// **'{count, plural, =0{No tasks} =1{1 task} other{{count} tasks}}'**
  String calendarDayTaskCount(int count);

  /// No description provided for @calendarCompletedTitle.
  ///
  /// In en, this message translates to:
  /// **'Completed'**
  String get calendarCompletedTitle;

  /// No description provided for @calendarShowCompletedTooltip.
  ///
  /// In en, this message translates to:
  /// **'Show completed work'**
  String get calendarShowCompletedTooltip;

  /// No description provided for @calendarHideCompletedTooltip.
  ///
  /// In en, this message translates to:
  /// **'Hide completed work'**
  String get calendarHideCompletedTooltip;

  /// No description provided for @calendarEmptyTitle.
  ///
  /// In en, this message translates to:
  /// **'Nothing planned.'**
  String get calendarEmptyTitle;

  /// No description provided for @calendarEmptyBody.
  ///
  /// In en, this message translates to:
  /// **'Choose another day or capture what you want to make time for.'**
  String get calendarEmptyBody;

  /// No description provided for @calendarLoadFailed.
  ///
  /// In en, this message translates to:
  /// **'Calendar could not be loaded.'**
  String get calendarLoadFailed;

  /// No description provided for @calendarLoadingSemantics.
  ///
  /// In en, this message translates to:
  /// **'Loading calendar'**
  String get calendarLoadingSemantics;

  /// No description provided for @calendarRetryButton.
  ///
  /// In en, this message translates to:
  /// **'Try again'**
  String get calendarRetryButton;

  /// No description provided for @calendarDueKind.
  ///
  /// In en, this message translates to:
  /// **'Due'**
  String get calendarDueKind;

  /// No description provided for @calendarScheduledKind.
  ///
  /// In en, this message translates to:
  /// **'Planned'**
  String get calendarScheduledKind;

  /// No description provided for @calendarCompletedKind.
  ///
  /// In en, this message translates to:
  /// **'Completed'**
  String get calendarCompletedKind;

  /// No description provided for @calendarArchivedListContext.
  ///
  /// In en, this message translates to:
  /// **'{listName} · Archived'**
  String calendarArchivedListContext(String listName);

  /// No description provided for @calendarOccurrenceSemantics.
  ///
  /// In en, this message translates to:
  /// **'{title}, {listName}, {kind}, {time}'**
  String calendarOccurrenceSemantics(
    String title,
    String listName,
    String kind,
    String time,
  );

  /// No description provided for @calendarMoveDueTooltip.
  ///
  /// In en, this message translates to:
  /// **'Move due date'**
  String get calendarMoveDueTooltip;

  /// No description provided for @calendarMoveScheduledTooltip.
  ///
  /// In en, this message translates to:
  /// **'Move planned date'**
  String get calendarMoveScheduledTooltip;

  /// No description provided for @calendarMoveOccurrenceSemantics.
  ///
  /// In en, this message translates to:
  /// **'{kind}: change date for {title}'**
  String calendarMoveOccurrenceSemantics(String kind, String title);

  /// No description provided for @calendarMoveSheetTitle.
  ///
  /// In en, this message translates to:
  /// **'Change date'**
  String get calendarMoveSheetTitle;

  /// No description provided for @calendarMoveToToday.
  ///
  /// In en, this message translates to:
  /// **'Today'**
  String get calendarMoveToToday;

  /// No description provided for @calendarMoveToTomorrow.
  ///
  /// In en, this message translates to:
  /// **'Tomorrow'**
  String get calendarMoveToTomorrow;

  /// No description provided for @calendarPickDate.
  ///
  /// In en, this message translates to:
  /// **'Choose date…'**
  String get calendarPickDate;

  /// No description provided for @todayTitle.
  ///
  /// In en, this message translates to:
  /// **'Today'**
  String get todayTitle;

  /// No description provided for @homeOverdueSectionTitle.
  ///
  /// In en, this message translates to:
  /// **'Overdue'**
  String get homeOverdueSectionTitle;

  /// No description provided for @homeTomorrowSectionTitle.
  ///
  /// In en, this message translates to:
  /// **'Tomorrow'**
  String get homeTomorrowSectionTitle;

  /// No description provided for @homeUpcomingSectionTitle.
  ///
  /// In en, this message translates to:
  /// **'Upcoming'**
  String get homeUpcomingSectionTitle;

  /// No description provided for @homeTasksSectionTitle.
  ///
  /// In en, this message translates to:
  /// **'Tasks'**
  String get homeTasksSectionTitle;

  /// No description provided for @homePendingCount.
  ///
  /// In en, this message translates to:
  /// **'{count} pending'**
  String homePendingCount(int count);

  /// No description provided for @completedTasksTitle.
  ///
  /// In en, this message translates to:
  /// **'Closed'**
  String get completedTasksTitle;

  /// No description provided for @showCompletedTasksTooltip.
  ///
  /// In en, this message translates to:
  /// **'Show closed tasks'**
  String get showCompletedTasksTooltip;

  /// No description provided for @hideCompletedTasksTooltip.
  ///
  /// In en, this message translates to:
  /// **'Hide closed tasks'**
  String get hideCompletedTasksTooltip;

  /// No description provided for @homeListMenuTooltip.
  ///
  /// In en, this message translates to:
  /// **'Open lists'**
  String get homeListMenuTooltip;

  /// No description provided for @homeSmartListTooltip.
  ///
  /// In en, this message translates to:
  /// **'Open Home'**
  String get homeSmartListTooltip;

  /// No description provided for @openSearchTooltip.
  ///
  /// In en, this message translates to:
  /// **'Search tasks'**
  String get openSearchTooltip;

  /// No description provided for @searchFieldHint.
  ///
  /// In en, this message translates to:
  /// **'Search tasks and notes'**
  String get searchFieldHint;

  /// No description provided for @searchFieldSemantics.
  ///
  /// In en, this message translates to:
  /// **'Search tasks and notes'**
  String get searchFieldSemantics;

  /// No description provided for @clearSearchTooltip.
  ///
  /// In en, this message translates to:
  /// **'Clear search'**
  String get clearSearchTooltip;

  /// No description provided for @searchEmptyTitle.
  ///
  /// In en, this message translates to:
  /// **'Find what you need.'**
  String get searchEmptyTitle;

  /// No description provided for @searchEmptyBody.
  ///
  /// In en, this message translates to:
  /// **'Search task titles and notes across every list.'**
  String get searchEmptyBody;

  /// No description provided for @searchNoResultsTitle.
  ///
  /// In en, this message translates to:
  /// **'Nothing found.'**
  String get searchNoResultsTitle;

  /// No description provided for @searchNoResultsBody.
  ///
  /// In en, this message translates to:
  /// **'No tasks match “{query}”.'**
  String searchNoResultsBody(String query);

  /// No description provided for @searchFailed.
  ///
  /// In en, this message translates to:
  /// **'Search could not be completed.'**
  String get searchFailed;

  /// No description provided for @searchLoadingSemantics.
  ///
  /// In en, this message translates to:
  /// **'Searching tasks'**
  String get searchLoadingSemantics;

  /// No description provided for @searchArchivedListLabel.
  ///
  /// In en, this message translates to:
  /// **'{listName} · Archived'**
  String searchArchivedListLabel(String listName);

  /// No description provided for @searchResultSemantics.
  ///
  /// In en, this message translates to:
  /// **'{title}, {listName}, {status}'**
  String searchResultSemantics(String title, String listName, String status);

  /// No description provided for @showHomeSectionTooltip.
  ///
  /// In en, this message translates to:
  /// **'Show {section} tasks'**
  String showHomeSectionTooltip(String section);

  /// No description provided for @hideHomeSectionTooltip.
  ///
  /// In en, this message translates to:
  /// **'Hide {section} tasks'**
  String hideHomeSectionTooltip(String section);

  /// No description provided for @homeEmptyTitle.
  ///
  /// In en, this message translates to:
  /// **'Start with a list.'**
  String get homeEmptyTitle;

  /// No description provided for @homeEmptyBody.
  ///
  /// In en, this message translates to:
  /// **'Create a list, then Taskveil will open straight into your tasks.'**
  String get homeEmptyBody;

  /// No description provided for @homeNewListButton.
  ///
  /// In en, this message translates to:
  /// **'New list'**
  String get homeNewListButton;

  /// No description provided for @homeClearTitle.
  ///
  /// In en, this message translates to:
  /// **'A little room to breathe.'**
  String get homeClearTitle;

  /// No description provided for @homeClearBody.
  ///
  /// In en, this message translates to:
  /// **'Nothing is waiting here. Add a task whenever you are ready.'**
  String get homeClearBody;

  /// No description provided for @addTaskButton.
  ///
  /// In en, this message translates to:
  /// **'Add task'**
  String get addTaskButton;

  /// No description provided for @quickAddHint.
  ///
  /// In en, this message translates to:
  /// **'Add task'**
  String get quickAddHint;

  /// No description provided for @quickAddOpenTooltip.
  ///
  /// In en, this message translates to:
  /// **'Open task creation'**
  String get quickAddOpenTooltip;

  /// No description provided for @quickAddOpenSemantics.
  ///
  /// In en, this message translates to:
  /// **'Open task creation sheet'**
  String get quickAddOpenSemantics;

  /// No description provided for @quickAddSubmitTooltip.
  ///
  /// In en, this message translates to:
  /// **'Add task'**
  String get quickAddSubmitTooltip;

  /// No description provided for @quickAddTextFieldSemantics.
  ///
  /// In en, this message translates to:
  /// **'Quick add task title'**
  String get quickAddTextFieldSemantics;

  /// No description provided for @quickAddCreateError.
  ///
  /// In en, this message translates to:
  /// **'Could not add the task.'**
  String get quickAddCreateError;

  /// No description provided for @taskCreateTitleHint.
  ///
  /// In en, this message translates to:
  /// **'Add a task...'**
  String get taskCreateTitleHint;

  /// No description provided for @taskCreateListChip.
  ///
  /// In en, this message translates to:
  /// **'List'**
  String get taskCreateListChip;

  /// No description provided for @taskCreateListTooltip.
  ///
  /// In en, this message translates to:
  /// **'Choose list'**
  String get taskCreateListTooltip;

  /// No description provided for @taskCreateDueChip.
  ///
  /// In en, this message translates to:
  /// **'Due'**
  String get taskCreateDueChip;

  /// No description provided for @taskCreateDueTooltip.
  ///
  /// In en, this message translates to:
  /// **'Choose due date'**
  String get taskCreateDueTooltip;

  /// No description provided for @taskCreateDueChipSemantics.
  ///
  /// In en, this message translates to:
  /// **'Due: {dueAt}'**
  String taskCreateDueChipSemantics(String dueAt);

  /// No description provided for @taskCreatePlanLabel.
  ///
  /// In en, this message translates to:
  /// **'Plan'**
  String get taskCreatePlanLabel;

  /// No description provided for @taskCreatePlanTooltip.
  ///
  /// In en, this message translates to:
  /// **'Set planned start and estimate'**
  String get taskCreatePlanTooltip;

  /// No description provided for @taskCreatePriorityTooltip.
  ///
  /// In en, this message translates to:
  /// **'Choose priority'**
  String get taskCreatePriorityTooltip;

  /// No description provided for @planNotSet.
  ///
  /// In en, this message translates to:
  /// **'Not planned'**
  String get planNotSet;

  /// No description provided for @planSheetTitle.
  ///
  /// In en, this message translates to:
  /// **'Plan'**
  String get planSheetTitle;

  /// No description provided for @plannedStartLabel.
  ///
  /// In en, this message translates to:
  /// **'Planned start'**
  String get plannedStartLabel;

  /// No description provided for @setPlannedStartButton.
  ///
  /// In en, this message translates to:
  /// **'Set date and time'**
  String get setPlannedStartButton;

  /// No description provided for @estimateLabel.
  ///
  /// In en, this message translates to:
  /// **'Estimate'**
  String get estimateLabel;

  /// No description provided for @estimateNotSet.
  ///
  /// In en, this message translates to:
  /// **'No estimate'**
  String get estimateNotSet;

  /// No description provided for @estimateMinutes.
  ///
  /// In en, this message translates to:
  /// **'{minutes} min'**
  String estimateMinutes(int minutes);

  /// No description provided for @decreaseEstimateTooltip.
  ///
  /// In en, this message translates to:
  /// **'Decrease estimate by 5 minutes'**
  String get decreaseEstimateTooltip;

  /// No description provided for @increaseEstimateTooltip.
  ///
  /// In en, this message translates to:
  /// **'Increase estimate by 5 minutes'**
  String get increaseEstimateTooltip;

  /// No description provided for @clearPlanButton.
  ///
  /// In en, this message translates to:
  /// **'Clear plan'**
  String get clearPlanButton;

  /// No description provided for @planSaveButton.
  ///
  /// In en, this message translates to:
  /// **'Apply plan'**
  String get planSaveButton;

  /// No description provided for @prioritySheetTitle.
  ///
  /// In en, this message translates to:
  /// **'Priority'**
  String get prioritySheetTitle;

  /// No description provided for @selectedOptionSemantics.
  ///
  /// In en, this message translates to:
  /// **'Selected: {option}'**
  String selectedOptionSemantics(String option);

  /// No description provided for @tasksEmpty.
  ///
  /// In en, this message translates to:
  /// **'No tasks yet. Tap + to create one.'**
  String get tasksEmpty;

  /// No description provided for @tasksEmptyTitle.
  ///
  /// In en, this message translates to:
  /// **'No tasks yet.'**
  String get tasksEmptyTitle;

  /// No description provided for @tasksEmptyBody.
  ///
  /// In en, this message translates to:
  /// **'Tap + to create one.'**
  String get tasksEmptyBody;

  /// No description provided for @failedToLoadTasks.
  ///
  /// In en, this message translates to:
  /// **'Failed to load tasks: {error}'**
  String failedToLoadTasks(String error);

  /// No description provided for @newTaskTooltip.
  ///
  /// In en, this message translates to:
  /// **'New task'**
  String get newTaskTooltip;

  /// No description provided for @newTaskTitle.
  ///
  /// In en, this message translates to:
  /// **'New task'**
  String get newTaskTitle;

  /// No description provided for @titleLabel.
  ///
  /// In en, this message translates to:
  /// **'Title'**
  String get titleLabel;

  /// No description provided for @noteLabel.
  ///
  /// In en, this message translates to:
  /// **'Note'**
  String get noteLabel;

  /// No description provided for @taskDetailTitle.
  ///
  /// In en, this message translates to:
  /// **'Task detail'**
  String get taskDetailTitle;

  /// No description provided for @failedToLoadTask.
  ///
  /// In en, this message translates to:
  /// **'Failed to load task: {error}'**
  String failedToLoadTask(String error);

  /// No description provided for @taskNotFound.
  ///
  /// In en, this message translates to:
  /// **'Task not found.'**
  String get taskNotFound;

  /// No description provided for @taskPriority.
  ///
  /// In en, this message translates to:
  /// **'Priority: {priority}'**
  String taskPriority(String priority);

  /// No description provided for @taskDueAt.
  ///
  /// In en, this message translates to:
  /// **'{dueAt}'**
  String taskDueAt(String dueAt);

  /// No description provided for @taskRowStatusSemantics.
  ///
  /// In en, this message translates to:
  /// **'Status: {status}'**
  String taskRowStatusSemantics(String status);

  /// No description provided for @taskRowDueSemantics.
  ///
  /// In en, this message translates to:
  /// **'Due: {dueAt}'**
  String taskRowDueSemantics(String dueAt);

  /// No description provided for @taskRowListSemantics.
  ///
  /// In en, this message translates to:
  /// **'List: {listName}'**
  String taskRowListSemantics(String listName);

  /// No description provided for @taskRowSubtaskLevelSemantics.
  ///
  /// In en, this message translates to:
  /// **'Subtask level {level}'**
  String taskRowSubtaskLevelSemantics(int level);

  /// No description provided for @taskRowOpenHint.
  ///
  /// In en, this message translates to:
  /// **'Double tap to open task'**
  String get taskRowOpenHint;

  /// No description provided for @dueToday.
  ///
  /// In en, this message translates to:
  /// **'Today'**
  String get dueToday;

  /// No description provided for @dueTomorrow.
  ///
  /// In en, this message translates to:
  /// **'Tomorrow'**
  String get dueTomorrow;

  /// No description provided for @taskDueOverdue.
  ///
  /// In en, this message translates to:
  /// **'Overdue: {dueAt}'**
  String taskDueOverdue(String dueAt);

  /// No description provided for @taskCreatedAt.
  ///
  /// In en, this message translates to:
  /// **'Created at: {createdAt}'**
  String taskCreatedAt(String createdAt);

  /// No description provided for @addNotePlaceholder.
  ///
  /// In en, this message translates to:
  /// **'Add note'**
  String get addNotePlaceholder;

  /// No description provided for @editTaskTitleSemantics.
  ///
  /// In en, this message translates to:
  /// **'Edit task title'**
  String get editTaskTitleSemantics;

  /// No description provided for @editTaskNoteSemantics.
  ///
  /// In en, this message translates to:
  /// **'Edit task note'**
  String get editTaskNoteSemantics;

  /// No description provided for @parentTaskLinkTooltip.
  ///
  /// In en, this message translates to:
  /// **'Open parent task: {title}'**
  String parentTaskLinkTooltip(String title);

  /// No description provided for @parentTaskLinkSemantics.
  ///
  /// In en, this message translates to:
  /// **'Parent task: {title}'**
  String parentTaskLinkSemantics(String title);

  /// No description provided for @changeDueDateTooltip.
  ///
  /// In en, this message translates to:
  /// **'Change due date'**
  String get changeDueDateTooltip;

  /// No description provided for @reminderChipEmpty.
  ///
  /// In en, this message translates to:
  /// **'Reminder'**
  String get reminderChipEmpty;

  /// No description provided for @reminderChipTooltipSet.
  ///
  /// In en, this message translates to:
  /// **'Set reminder'**
  String get reminderChipTooltipSet;

  /// No description provided for @reminderChipTooltipChange.
  ///
  /// In en, this message translates to:
  /// **'Manage reminders'**
  String get reminderChipTooltipChange;

  /// No description provided for @clearReminderButton.
  ///
  /// In en, this message translates to:
  /// **'Clear reminder'**
  String get clearReminderButton;

  /// No description provided for @manageRemindersTitle.
  ///
  /// In en, this message translates to:
  /// **'Reminders'**
  String get manageRemindersTitle;

  /// No description provided for @addReminderButton.
  ///
  /// In en, this message translates to:
  /// **'Add reminder'**
  String get addReminderButton;

  /// No description provided for @editReminderTooltip.
  ///
  /// In en, this message translates to:
  /// **'Edit reminder'**
  String get editReminderTooltip;

  /// No description provided for @deleteReminderTooltip.
  ///
  /// In en, this message translates to:
  /// **'Delete reminder'**
  String get deleteReminderTooltip;

  /// No description provided for @reminderCount.
  ///
  /// In en, this message translates to:
  /// **'{count} reminders'**
  String reminderCount(int count);

  /// No description provided for @reminderQuickOptionsTitle.
  ///
  /// In en, this message translates to:
  /// **'Before due time'**
  String get reminderQuickOptionsTitle;

  /// No description provided for @reminderFiveMinutesBefore.
  ///
  /// In en, this message translates to:
  /// **'5 minutes before'**
  String get reminderFiveMinutesBefore;

  /// No description provided for @reminderThirtyMinutesBefore.
  ///
  /// In en, this message translates to:
  /// **'30 minutes before'**
  String get reminderThirtyMinutesBefore;

  /// No description provided for @reminderOneHourBefore.
  ///
  /// In en, this message translates to:
  /// **'1 hour before'**
  String get reminderOneHourBefore;

  /// No description provided for @reminderOneDayBefore.
  ///
  /// In en, this message translates to:
  /// **'1 day before'**
  String get reminderOneDayBefore;

  /// No description provided for @reminderCustomTime.
  ///
  /// In en, this message translates to:
  /// **'Custom date and time'**
  String get reminderCustomTime;

  /// No description provided for @reminderPastLabel.
  ///
  /// In en, this message translates to:
  /// **'Past'**
  String get reminderPastLabel;

  /// No description provided for @reminderLimitReached.
  ///
  /// In en, this message translates to:
  /// **'You can set up to 5 reminders for one task.'**
  String get reminderLimitReached;

  /// No description provided for @reminderDuplicateTime.
  ///
  /// In en, this message translates to:
  /// **'A reminder already exists at that time.'**
  String get reminderDuplicateTime;

  /// No description provided for @reminderMustBeFuture.
  ///
  /// In en, this message translates to:
  /// **'Choose a future time.'**
  String get reminderMustBeFuture;

  /// No description provided for @reminderSavedNotificationFailed.
  ///
  /// In en, this message translates to:
  /// **'The reminder was saved, but Taskveil could not schedule the local notification.'**
  String get reminderSavedNotificationFailed;

  /// No description provided for @reminderPermissionDenied.
  ///
  /// In en, this message translates to:
  /// **'Notifications are off. The reminder was saved, but Taskveil could not schedule a local notification.'**
  String get reminderPermissionDenied;

  /// No description provided for @failedToSaveReminder.
  ///
  /// In en, this message translates to:
  /// **'Failed to save reminder: {error}'**
  String failedToSaveReminder(String error);

  /// No description provided for @reminderNotificationTitle.
  ///
  /// In en, this message translates to:
  /// **'Taskveil reminder'**
  String get reminderNotificationTitle;

  /// No description provided for @reminderNotificationBody.
  ///
  /// In en, this message translates to:
  /// **'A task reminder is due.'**
  String get reminderNotificationBody;

  /// No description provided for @reminderSnoozeOneHourAction.
  ///
  /// In en, this message translates to:
  /// **'+1 hour'**
  String get reminderSnoozeOneHourAction;

  /// No description provided for @changePriorityTooltip.
  ///
  /// In en, this message translates to:
  /// **'Change priority'**
  String get changePriorityTooltip;

  /// No description provided for @subtasksTitle.
  ///
  /// In en, this message translates to:
  /// **'Subtasks'**
  String get subtasksTitle;

  /// No description provided for @subtasksEmpty.
  ///
  /// In en, this message translates to:
  /// **'No subtasks yet.'**
  String get subtasksEmpty;

  /// No description provided for @addSubtaskButton.
  ///
  /// In en, this message translates to:
  /// **'Add subtask'**
  String get addSubtaskButton;

  /// No description provided for @newSubtaskTitle.
  ///
  /// In en, this message translates to:
  /// **'New subtask'**
  String get newSubtaskTitle;

  /// No description provided for @subtaskProgress.
  ///
  /// In en, this message translates to:
  /// **'{doneCount}/{totalCount}'**
  String subtaskProgress(int doneCount, int totalCount);

  /// No description provided for @completeTaskDialogTitle.
  ///
  /// In en, this message translates to:
  /// **'Complete parent task?'**
  String get completeTaskDialogTitle;

  /// No description provided for @completeTaskDialogMessage.
  ///
  /// In en, this message translates to:
  /// **'This task has incomplete subtasks. Completing it will not complete its subtasks.'**
  String get completeTaskDialogMessage;

  /// No description provided for @wontDoTaskDialogTitle.
  ///
  /// In en, this message translates to:
  /// **'Close parent as won\'t do?'**
  String get wontDoTaskDialogTitle;

  /// No description provided for @wontDoTaskDialogMessage.
  ///
  /// In en, this message translates to:
  /// **'This task has incomplete subtasks. Closing it as won\'t do will not close its subtasks.'**
  String get wontDoTaskDialogMessage;

  /// No description provided for @continueButton.
  ///
  /// In en, this message translates to:
  /// **'Continue'**
  String get continueButton;

  /// No description provided for @statusTodo.
  ///
  /// In en, this message translates to:
  /// **'To do'**
  String get statusTodo;

  /// No description provided for @statusInProgress.
  ///
  /// In en, this message translates to:
  /// **'In progress'**
  String get statusInProgress;

  /// No description provided for @statusDone.
  ///
  /// In en, this message translates to:
  /// **'Done'**
  String get statusDone;

  /// No description provided for @statusWontDo.
  ///
  /// In en, this message translates to:
  /// **'Won\'t do'**
  String get statusWontDo;

  /// No description provided for @editTaskTooltip.
  ///
  /// In en, this message translates to:
  /// **'Edit task'**
  String get editTaskTooltip;

  /// No description provided for @taskActionsTooltip.
  ///
  /// In en, this message translates to:
  /// **'Task actions'**
  String get taskActionsTooltip;

  /// No description provided for @completeTaskTooltip.
  ///
  /// In en, this message translates to:
  /// **'Mark task done'**
  String get completeTaskTooltip;

  /// No description provided for @markTaskDoneMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Mark done'**
  String get markTaskDoneMenuItem;

  /// No description provided for @markTaskWontDoMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Mark won\'t do'**
  String get markTaskWontDoMenuItem;

  /// No description provided for @reopenTaskTooltip.
  ///
  /// In en, this message translates to:
  /// **'Reopen task'**
  String get reopenTaskTooltip;

  /// No description provided for @reopenTaskMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Reopen'**
  String get reopenTaskMenuItem;

  /// No description provided for @editTaskTitle.
  ///
  /// In en, this message translates to:
  /// **'Edit task'**
  String get editTaskTitle;

  /// No description provided for @priorityLabel.
  ///
  /// In en, this message translates to:
  /// **'Priority'**
  String get priorityLabel;

  /// No description provided for @priorityNone.
  ///
  /// In en, this message translates to:
  /// **'None'**
  String get priorityNone;

  /// No description provided for @priorityLow.
  ///
  /// In en, this message translates to:
  /// **'Low'**
  String get priorityLow;

  /// No description provided for @priorityMedium.
  ///
  /// In en, this message translates to:
  /// **'Medium'**
  String get priorityMedium;

  /// No description provided for @priorityHigh.
  ///
  /// In en, this message translates to:
  /// **'High'**
  String get priorityHigh;

  /// No description provided for @dueDateLabel.
  ///
  /// In en, this message translates to:
  /// **'Due'**
  String get dueDateLabel;

  /// No description provided for @noDueDate.
  ///
  /// In en, this message translates to:
  /// **'No due date'**
  String get noDueDate;

  /// No description provided for @setDueDateButton.
  ///
  /// In en, this message translates to:
  /// **'Set date'**
  String get setDueDateButton;

  /// No description provided for @setDueDateTimeButton.
  ///
  /// In en, this message translates to:
  /// **'Set date and time'**
  String get setDueDateTimeButton;

  /// No description provided for @clearDueDateButton.
  ///
  /// In en, this message translates to:
  /// **'Clear date'**
  String get clearDueDateButton;

  /// No description provided for @saveButton.
  ///
  /// In en, this message translates to:
  /// **'Save'**
  String get saveButton;

  /// No description provided for @titleRequiredError.
  ///
  /// In en, this message translates to:
  /// **'Title is required.'**
  String get titleRequiredError;

  /// No description provided for @failedToSaveTask.
  ///
  /// In en, this message translates to:
  /// **'Failed to save task: {error}'**
  String failedToSaveTask(String error);

  /// No description provided for @deleteTaskMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Delete'**
  String get deleteTaskMenuItem;

  /// No description provided for @deleteTaskDialogTitle.
  ///
  /// In en, this message translates to:
  /// **'Delete task?'**
  String get deleteTaskDialogTitle;

  /// No description provided for @deleteTaskDialogMessage.
  ///
  /// In en, this message translates to:
  /// **'This task will be permanently deleted and cannot be recovered.'**
  String get deleteTaskDialogMessage;

  /// No description provided for @deleteTaskDialogMessageWithDescendants.
  ///
  /// In en, this message translates to:
  /// **'This task and {descendantCount} subtasks will be permanently deleted and cannot be recovered.'**
  String deleteTaskDialogMessageWithDescendants(int descendantCount);

  /// No description provided for @undoActionLabel.
  ///
  /// In en, this message translates to:
  /// **'Undo'**
  String get undoActionLabel;

  /// No description provided for @undoCompleteMessage.
  ///
  /// In en, this message translates to:
  /// **'Task completed.'**
  String get undoCompleteMessage;

  /// No description provided for @undoCloseMessage.
  ///
  /// In en, this message translates to:
  /// **'Task closed.'**
  String get undoCloseMessage;

  /// No description provided for @undoEditMessage.
  ///
  /// In en, this message translates to:
  /// **'Task saved.'**
  String get undoEditMessage;

  /// No description provided for @undoSuccessMessage.
  ///
  /// In en, this message translates to:
  /// **'Undone.'**
  String get undoSuccessMessage;

  /// No description provided for @undoFailedMessage.
  ///
  /// In en, this message translates to:
  /// **'Undo failed: {error}'**
  String undoFailedMessage(String error);

  /// No description provided for @taskSortTooltip.
  ///
  /// In en, this message translates to:
  /// **'Sort tasks'**
  String get taskSortTooltip;

  /// No description provided for @taskSortManual.
  ///
  /// In en, this message translates to:
  /// **'Manual'**
  String get taskSortManual;

  /// No description provided for @taskSortDueDate.
  ///
  /// In en, this message translates to:
  /// **'Due date'**
  String get taskSortDueDate;

  /// No description provided for @taskSortPriority.
  ///
  /// In en, this message translates to:
  /// **'Priority'**
  String get taskSortPriority;

  /// No description provided for @taskSortCreatedAt.
  ///
  /// In en, this message translates to:
  /// **'Created'**
  String get taskSortCreatedAt;

  /// No description provided for @moveTaskUpTooltip.
  ///
  /// In en, this message translates to:
  /// **'Move task up'**
  String get moveTaskUpTooltip;

  /// No description provided for @moveTaskDownTooltip.
  ///
  /// In en, this message translates to:
  /// **'Move task down'**
  String get moveTaskDownTooltip;

  /// No description provided for @accountTitle.
  ///
  /// In en, this message translates to:
  /// **'Account'**
  String get accountTitle;

  /// No description provided for @accountSubtitle.
  ///
  /// In en, this message translates to:
  /// **'Private sync, security, and account settings.'**
  String get accountSubtitle;

  /// No description provided for @navigationMenuLabel.
  ///
  /// In en, this message translates to:
  /// **'Menu'**
  String get navigationMenuLabel;

  /// No description provided for @menuTitle.
  ///
  /// In en, this message translates to:
  /// **'Menu'**
  String get menuTitle;

  /// No description provided for @menuSubtitle.
  ///
  /// In en, this message translates to:
  /// **'Your workspace, account, and reusable tools.'**
  String get menuSubtitle;

  /// No description provided for @menuSectionTitle.
  ///
  /// In en, this message translates to:
  /// **'WORKSPACE'**
  String get menuSectionTitle;

  /// No description provided for @menuAccountBody.
  ///
  /// In en, this message translates to:
  /// **'Sign in, sync, security, and Pro'**
  String get menuAccountBody;

  /// No description provided for @menuTemplatesBody.
  ///
  /// In en, this message translates to:
  /// **'Reusable and recurring tasks'**
  String get menuTemplatesBody;

  /// No description provided for @calendarSettingsTitle.
  ///
  /// In en, this message translates to:
  /// **'Calendar settings'**
  String get calendarSettingsTitle;

  /// No description provided for @calendarSettingsSubtitle.
  ///
  /// In en, this message translates to:
  /// **'Choose how weeks are arranged across the calendar.'**
  String get calendarSettingsSubtitle;

  /// No description provided for @calendarWeekStartSectionTitle.
  ///
  /// In en, this message translates to:
  /// **'WEEK STARTS ON'**
  String get calendarWeekStartSectionTitle;

  /// No description provided for @calendarWeekStartSystem.
  ///
  /// In en, this message translates to:
  /// **'Region default'**
  String get calendarWeekStartSystem;

  /// No description provided for @calendarWeekStartSystemBody.
  ///
  /// In en, this message translates to:
  /// **'Use the first day of the week from this device\'s region.'**
  String get calendarWeekStartSystemBody;

  /// No description provided for @calendarWeekStartMonday.
  ///
  /// In en, this message translates to:
  /// **'Monday'**
  String get calendarWeekStartMonday;

  /// No description provided for @calendarWeekStartMondayBody.
  ///
  /// In en, this message translates to:
  /// **'Show Monday in the first column.'**
  String get calendarWeekStartMondayBody;

  /// No description provided for @calendarWeekStartSunday.
  ///
  /// In en, this message translates to:
  /// **'Sunday'**
  String get calendarWeekStartSunday;

  /// No description provided for @calendarWeekStartSundayBody.
  ///
  /// In en, this message translates to:
  /// **'Show Sunday in the first column.'**
  String get calendarWeekStartSundayBody;

  /// No description provided for @calendarSettingsLoadFailed.
  ///
  /// In en, this message translates to:
  /// **'Could not load calendar settings.'**
  String get calendarSettingsLoadFailed;

  /// No description provided for @calendarSettingsSaveFailed.
  ///
  /// In en, this message translates to:
  /// **'Could not save the calendar setting. Try again.'**
  String get calendarSettingsSaveFailed;

  /// No description provided for @accountPrivateSectionTitle.
  ///
  /// In en, this message translates to:
  /// **'PRIVATE ACCOUNT'**
  String get accountPrivateSectionTitle;

  /// No description provided for @accountPrivateTitle.
  ///
  /// In en, this message translates to:
  /// **'Your encrypted workspace'**
  String get accountPrivateTitle;

  /// No description provided for @accountPrivateBody.
  ///
  /// In en, this message translates to:
  /// **'Sign in to sync protected tasks across your devices.'**
  String get accountPrivateBody;

  /// No description provided for @accountEncryptionStatus.
  ///
  /// In en, this message translates to:
  /// **'End-to-end encrypted workspace'**
  String get accountEncryptionStatus;

  /// No description provided for @accountSecurityTitle.
  ///
  /// In en, this message translates to:
  /// **'SECURITY'**
  String get accountSecurityTitle;

  /// No description provided for @accountConnectionTitle.
  ///
  /// In en, this message translates to:
  /// **'CONNECTION'**
  String get accountConnectionTitle;

  /// No description provided for @accountConnectionBody.
  ///
  /// In en, this message translates to:
  /// **'Advanced sync server settings for this device.'**
  String get accountConnectionBody;

  /// No description provided for @accountLoadFailed.
  ///
  /// In en, this message translates to:
  /// **'Could not load account state.'**
  String get accountLoadFailed;

  /// No description provided for @accountLoginTab.
  ///
  /// In en, this message translates to:
  /// **'Log in'**
  String get accountLoginTab;

  /// No description provided for @accountRegisterTab.
  ///
  /// In en, this message translates to:
  /// **'Register'**
  String get accountRegisterTab;

  /// No description provided for @accountEmailLabel.
  ///
  /// In en, this message translates to:
  /// **'Email'**
  String get accountEmailLabel;

  /// No description provided for @accountPasswordLabel.
  ///
  /// In en, this message translates to:
  /// **'Password'**
  String get accountPasswordLabel;

  /// No description provided for @accountLoginButton.
  ///
  /// In en, this message translates to:
  /// **'Log in'**
  String get accountLoginButton;

  /// No description provided for @accountRegisterButton.
  ///
  /// In en, this message translates to:
  /// **'Register'**
  String get accountRegisterButton;

  /// No description provided for @accountVerificationPending.
  ///
  /// In en, this message translates to:
  /// **'Enter the 8-digit code sent to your email. This confirms mailbox access only; it cannot recover encrypted data.'**
  String get accountVerificationPending;

  /// No description provided for @accountVerificationCodeLabel.
  ///
  /// In en, this message translates to:
  /// **'8-digit verification code'**
  String get accountVerificationCodeLabel;

  /// No description provided for @accountVerifyCodeButton.
  ///
  /// In en, this message translates to:
  /// **'Verify code'**
  String get accountVerifyCodeButton;

  /// No description provided for @accountResendCodeButton.
  ///
  /// In en, this message translates to:
  /// **'Resend code'**
  String get accountResendCodeButton;

  /// No description provided for @accountResendAvailableIn.
  ///
  /// In en, this message translates to:
  /// **'You can resend in {seconds} seconds.'**
  String accountResendAvailableIn(int seconds);

  /// No description provided for @accountCancelRegistrationButton.
  ///
  /// In en, this message translates to:
  /// **'Cancel registration'**
  String get accountCancelRegistrationButton;

  /// No description provided for @accountSetPasswordButton.
  ///
  /// In en, this message translates to:
  /// **'Set password and finish'**
  String get accountSetPasswordButton;

  /// No description provided for @accountRecoveryAcknowledgeButton.
  ///
  /// In en, this message translates to:
  /// **'I saved my Recovery Key'**
  String get accountRecoveryAcknowledgeButton;

  /// No description provided for @accountLogoutButton.
  ///
  /// In en, this message translates to:
  /// **'Log out'**
  String get accountLogoutButton;

  /// No description provided for @accountServerUrlLabel.
  ///
  /// In en, this message translates to:
  /// **'Server URL'**
  String get accountServerUrlLabel;

  /// No description provided for @accountSaveServerUrlTooltip.
  ///
  /// In en, this message translates to:
  /// **'Save server URL'**
  String get accountSaveServerUrlTooltip;

  /// No description provided for @accountRequestFailed.
  ///
  /// In en, this message translates to:
  /// **'Account request failed.'**
  String get accountRequestFailed;

  /// No description provided for @accountSyncTitle.
  ///
  /// In en, this message translates to:
  /// **'Sync'**
  String get accountSyncTitle;

  /// No description provided for @accountSyncNotSignedIn.
  ///
  /// In en, this message translates to:
  /// **'Sync is off.'**
  String get accountSyncNotSignedIn;

  /// No description provided for @accountSyncIdle.
  ///
  /// In en, this message translates to:
  /// **'Ready'**
  String get accountSyncIdle;

  /// No description provided for @accountSyncRunning.
  ///
  /// In en, this message translates to:
  /// **'Syncing'**
  String get accountSyncRunning;

  /// No description provided for @accountSyncFailed.
  ///
  /// In en, this message translates to:
  /// **'Sync failed'**
  String get accountSyncFailed;

  /// No description provided for @accountSyncLastSuccess.
  ///
  /// In en, this message translates to:
  /// **'Last synced: {time}'**
  String accountSyncLastSuccess(String time);

  /// No description provided for @accountSyncNever.
  ///
  /// In en, this message translates to:
  /// **'Never'**
  String get accountSyncNever;

  /// No description provided for @accountSyncNowButton.
  ///
  /// In en, this message translates to:
  /// **'Sync now'**
  String get accountSyncNowButton;

  /// No description provided for @accountSyncNowTooltip.
  ///
  /// In en, this message translates to:
  /// **'Sync now'**
  String get accountSyncNowTooltip;

  /// No description provided for @organizationSafetyOpenButton.
  ///
  /// In en, this message translates to:
  /// **'Verify organization member'**
  String get organizationSafetyOpenButton;

  /// No description provided for @organizationSafetyTitle.
  ///
  /// In en, this message translates to:
  /// **'Safety number'**
  String get organizationSafetyTitle;

  /// No description provided for @organizationSafetyBody.
  ///
  /// In en, this message translates to:
  /// **'Compare this number or QR code with the other account through a separate trusted channel. Organization keys are not delivered until both accounts confirm the same value.'**
  String get organizationSafetyBody;

  /// No description provided for @organizationTenantIdLabel.
  ///
  /// In en, this message translates to:
  /// **'Organization ID'**
  String get organizationTenantIdLabel;

  /// No description provided for @organizationMemberIdLabel.
  ///
  /// In en, this message translates to:
  /// **'Member account ID'**
  String get organizationMemberIdLabel;

  /// No description provided for @organizationSafetyLoadButton.
  ///
  /// In en, this message translates to:
  /// **'Show Safety number'**
  String get organizationSafetyLoadButton;

  /// No description provided for @organizationSafetyQrSemantics.
  ///
  /// In en, this message translates to:
  /// **'Safety number QR code'**
  String get organizationSafetyQrSemantics;

  /// No description provided for @organizationSafetyVerified.
  ///
  /// In en, this message translates to:
  /// **'Verified by both accounts'**
  String get organizationSafetyVerified;

  /// No description provided for @organizationSafetyUnverified.
  ///
  /// In en, this message translates to:
  /// **'Not yet verified by both accounts'**
  String get organizationSafetyUnverified;

  /// No description provided for @organizationSafetyComparedOutOfBand.
  ///
  /// In en, this message translates to:
  /// **'I compared this value through a separate trusted channel'**
  String get organizationSafetyComparedOutOfBand;

  /// No description provided for @organizationSafetyConfirmButton.
  ///
  /// In en, this message translates to:
  /// **'Confirm this Safety number'**
  String get organizationSafetyConfirmButton;

  /// No description provided for @organizationSafetyFailed.
  ///
  /// In en, this message translates to:
  /// **'Could not verify the Safety number. Reload it and compare again.'**
  String get organizationSafetyFailed;

  /// No description provided for @onboardingWelcomeTitle.
  ///
  /// In en, this message translates to:
  /// **'Make room for what matters'**
  String get onboardingWelcomeTitle;

  /// No description provided for @onboardingWelcomeBody.
  ///
  /// In en, this message translates to:
  /// **'A calm place for plans, small promises, and the next thing worth doing. No scores. No noise.'**
  String get onboardingWelcomeBody;

  /// No description provided for @onboardingWelcomeArtworkSemantics.
  ///
  /// In en, this message translates to:
  /// **'A quiet leaf representing Taskveil'**
  String get onboardingWelcomeArtworkSemantics;

  /// No description provided for @onboardingPrivacyTitle.
  ///
  /// In en, this message translates to:
  /// **'Private by design'**
  String get onboardingPrivacyTitle;

  /// No description provided for @onboardingPrivacyBody.
  ///
  /// In en, this message translates to:
  /// **'Your local database is encrypted on this device. When you choose to sync, task content is encrypted before it leaves.'**
  String get onboardingPrivacyBody;

  /// No description provided for @onboardingPrivacyNote.
  ///
  /// In en, this message translates to:
  /// **'Without sync, your tasks live only on this device and may be unrecoverable if the device is lost or the app is removed.'**
  String get onboardingPrivacyNote;

  /// No description provided for @onboardingPrivacyArtworkSemantics.
  ///
  /// In en, this message translates to:
  /// **'A shield representing local protection and encrypted sync'**
  String get onboardingPrivacyArtworkSemantics;

  /// No description provided for @onboardingBeginTitle.
  ///
  /// In en, this message translates to:
  /// **'Begin with one small thing'**
  String get onboardingBeginTitle;

  /// No description provided for @onboardingBeginBody.
  ///
  /// In en, this message translates to:
  /// **'Add what needs your attention. Taskveil stays quiet until you need it.'**
  String get onboardingBeginBody;

  /// No description provided for @onboardingBeginArtworkSemantics.
  ///
  /// In en, this message translates to:
  /// **'A settled check mark representing a completed task'**
  String get onboardingBeginArtworkSemantics;

  /// No description provided for @onboardingStartButton.
  ///
  /// In en, this message translates to:
  /// **'Start gently'**
  String get onboardingStartButton;

  /// No description provided for @onboardingSaveFailed.
  ///
  /// In en, this message translates to:
  /// **'Taskveil couldn\'t save this choice. Try again to continue.'**
  String get onboardingSaveFailed;

  /// No description provided for @onboardingLoadFailed.
  ///
  /// In en, this message translates to:
  /// **'Taskveil couldn\'t read its local settings.'**
  String get onboardingLoadFailed;

  /// No description provided for @retryButton.
  ///
  /// In en, this message translates to:
  /// **'Try again'**
  String get retryButton;

  /// No description provided for @onboardingPagePosition.
  ///
  /// In en, this message translates to:
  /// **'Page {current} of {total}'**
  String onboardingPagePosition(int current, int total);

  /// No description provided for @focusTitle.
  ///
  /// In en, this message translates to:
  /// **'Focus'**
  String get focusTitle;

  /// No description provided for @focusSetupTitle.
  ///
  /// In en, this message translates to:
  /// **'Choose how to focus'**
  String get focusSetupTitle;

  /// No description provided for @focusSetupBody.
  ///
  /// In en, this message translates to:
  /// **'Stay with one task. The rest can wait.'**
  String get focusSetupBody;

  /// No description provided for @focusPomodoroMode.
  ///
  /// In en, this message translates to:
  /// **'Pomodoro'**
  String get focusPomodoroMode;

  /// No description provided for @focusStopwatchMode.
  ///
  /// In en, this message translates to:
  /// **'Stopwatch'**
  String get focusStopwatchMode;

  /// No description provided for @focusPomodoroSummary.
  ///
  /// In en, this message translates to:
  /// **'{work} min focus · {breakMinutes} min break'**
  String focusPomodoroSummary(int work, int breakMinutes);

  /// No description provided for @focusStopwatchSummary.
  ///
  /// In en, this message translates to:
  /// **'Open-ended, with pause and resume'**
  String get focusStopwatchSummary;

  /// No description provided for @focusStartButton.
  ///
  /// In en, this message translates to:
  /// **'Start focus'**
  String get focusStartButton;

  /// No description provided for @focusSettingsButton.
  ///
  /// In en, this message translates to:
  /// **'Pomodoro settings'**
  String get focusSettingsButton;

  /// No description provided for @focusSettingsTitle.
  ///
  /// In en, this message translates to:
  /// **'Pomodoro rhythm'**
  String get focusSettingsTitle;

  /// No description provided for @focusWorkMinutesLabel.
  ///
  /// In en, this message translates to:
  /// **'Focus'**
  String get focusWorkMinutesLabel;

  /// No description provided for @focusShortBreakMinutesLabel.
  ///
  /// In en, this message translates to:
  /// **'Short break'**
  String get focusShortBreakMinutesLabel;

  /// No description provided for @focusLongBreakMinutesLabel.
  ///
  /// In en, this message translates to:
  /// **'Long break'**
  String get focusLongBreakMinutesLabel;

  /// No description provided for @focusLongBreakEveryLabel.
  ///
  /// In en, this message translates to:
  /// **'Long break after'**
  String get focusLongBreakEveryLabel;

  /// No description provided for @focusNotificationsLabel.
  ///
  /// In en, this message translates to:
  /// **'Completion notification'**
  String get focusNotificationsLabel;

  /// No description provided for @focusNotificationsBody.
  ///
  /// In en, this message translates to:
  /// **'Best effort while Taskveil is in the background'**
  String get focusNotificationsBody;

  /// No description provided for @focusWorkIntervals.
  ///
  /// In en, this message translates to:
  /// **'{count} focus sessions'**
  String focusWorkIntervals(int count);

  /// No description provided for @focusRestoring.
  ///
  /// In en, this message translates to:
  /// **'Restoring your focus session…'**
  String get focusRestoring;

  /// No description provided for @focusLoadFailed.
  ///
  /// In en, this message translates to:
  /// **'Taskveil couldn\'t restore this focus session.'**
  String get focusLoadFailed;

  /// No description provided for @focusActiveConflictTitle.
  ///
  /// In en, this message translates to:
  /// **'Another focus session is active'**
  String get focusActiveConflictTitle;

  /// No description provided for @focusActiveConflictBody.
  ///
  /// In en, this message translates to:
  /// **'Finish or discard the current session before starting this task.'**
  String get focusActiveConflictBody;

  /// No description provided for @focusRunningState.
  ///
  /// In en, this message translates to:
  /// **'Focusing'**
  String get focusRunningState;

  /// No description provided for @focusPausedState.
  ///
  /// In en, this message translates to:
  /// **'Paused'**
  String get focusPausedState;

  /// No description provided for @focusWorkPhase.
  ///
  /// In en, this message translates to:
  /// **'Focus session'**
  String get focusWorkPhase;

  /// No description provided for @focusShortBreakPhase.
  ///
  /// In en, this message translates to:
  /// **'Short break'**
  String get focusShortBreakPhase;

  /// No description provided for @focusLongBreakPhase.
  ///
  /// In en, this message translates to:
  /// **'Long break'**
  String get focusLongBreakPhase;

  /// No description provided for @focusBreakPrompt.
  ///
  /// In en, this message translates to:
  /// **'Take a breath.'**
  String get focusBreakPrompt;

  /// No description provided for @focusElapsedLabel.
  ///
  /// In en, this message translates to:
  /// **'Elapsed {time}'**
  String focusElapsedLabel(String time);

  /// No description provided for @focusPauseButton.
  ///
  /// In en, this message translates to:
  /// **'Pause'**
  String get focusPauseButton;

  /// No description provided for @focusResumeButton.
  ///
  /// In en, this message translates to:
  /// **'Resume'**
  String get focusResumeButton;

  /// No description provided for @focusSessionOptionsButton.
  ///
  /// In en, this message translates to:
  /// **'Session options'**
  String get focusSessionOptionsButton;

  /// No description provided for @focusAddTimeButton.
  ///
  /// In en, this message translates to:
  /// **'Add 5 minutes'**
  String get focusAddTimeButton;

  /// No description provided for @focusFinishButton.
  ///
  /// In en, this message translates to:
  /// **'Finish'**
  String get focusFinishButton;

  /// No description provided for @focusFinishSessionButton.
  ///
  /// In en, this message translates to:
  /// **'Finish session'**
  String get focusFinishSessionButton;

  /// No description provided for @focusEndBreakButton.
  ///
  /// In en, this message translates to:
  /// **'End break'**
  String get focusEndBreakButton;

  /// No description provided for @focusSaveAndExitButton.
  ///
  /// In en, this message translates to:
  /// **'Save and exit'**
  String get focusSaveAndExitButton;

  /// No description provided for @focusDiscardButton.
  ///
  /// In en, this message translates to:
  /// **'Discard'**
  String get focusDiscardButton;

  /// No description provided for @focusDiscardTitle.
  ///
  /// In en, this message translates to:
  /// **'Discard this session?'**
  String get focusDiscardTitle;

  /// No description provided for @focusDiscardBody.
  ///
  /// In en, this message translates to:
  /// **'This session will end without saving work time.'**
  String get focusDiscardBody;

  /// No description provided for @focusCompleteTaskButton.
  ///
  /// In en, this message translates to:
  /// **'Complete task'**
  String get focusCompleteTaskButton;

  /// No description provided for @focusFinishedTitle.
  ///
  /// In en, this message translates to:
  /// **'Focus recorded'**
  String get focusFinishedTitle;

  /// No description provided for @focusFinishedSummary.
  ///
  /// In en, this message translates to:
  /// **'Focused work recorded.'**
  String get focusFinishedSummary;

  /// No description provided for @focusFinishedBody.
  ///
  /// In en, this message translates to:
  /// **'{time} of focused work recorded.'**
  String focusFinishedBody(String time);

  /// No description provided for @focusBreakFinishedTitle.
  ///
  /// In en, this message translates to:
  /// **'Break complete'**
  String get focusBreakFinishedTitle;

  /// No description provided for @focusBreakFinishedBody.
  ///
  /// In en, this message translates to:
  /// **'Return when you\'re ready for the next focus session.'**
  String get focusBreakFinishedBody;

  /// No description provided for @focusStartBreakButton.
  ///
  /// In en, this message translates to:
  /// **'Start break'**
  String get focusStartBreakButton;

  /// No description provided for @focusKeepSessionButton.
  ///
  /// In en, this message translates to:
  /// **'Keep focusing'**
  String get focusKeepSessionButton;

  /// No description provided for @focusDoneButton.
  ///
  /// In en, this message translates to:
  /// **'Done'**
  String get focusDoneButton;

  /// No description provided for @focusActionFailed.
  ///
  /// In en, this message translates to:
  /// **'Couldn\'t update this focus session. Try again.'**
  String get focusActionFailed;

  /// No description provided for @focusTaskCompleteFailed.
  ///
  /// In en, this message translates to:
  /// **'The session was saved, but the task could not be completed. Try completing it again.'**
  String get focusTaskCompleteFailed;

  /// No description provided for @focusEstimateActualLabel.
  ///
  /// In en, this message translates to:
  /// **'Focus time'**
  String get focusEstimateActualLabel;

  /// No description provided for @focusEstimateActualValue.
  ///
  /// In en, this message translates to:
  /// **'{actual} actual · {estimate} estimated'**
  String focusEstimateActualValue(String actual, String estimate);

  /// No description provided for @focusActualOnlyValue.
  ///
  /// In en, this message translates to:
  /// **'{actual} recorded'**
  String focusActualOnlyValue(String actual);

  /// No description provided for @focusNoActualValue.
  ///
  /// In en, this message translates to:
  /// **'No work recorded yet'**
  String get focusNoActualValue;

  /// No description provided for @timerNotificationTitle.
  ///
  /// In en, this message translates to:
  /// **'Focus time is complete'**
  String get timerNotificationTitle;

  /// No description provided for @timerNotificationBody.
  ///
  /// In en, this message translates to:
  /// **'Open Taskveil to continue your focus rhythm.'**
  String get timerNotificationBody;

  /// No description provided for @billingTitle.
  ///
  /// In en, this message translates to:
  /// **'Pro'**
  String get billingTitle;

  /// No description provided for @billingSubscriptionBody.
  ///
  /// In en, this message translates to:
  /// **'Pro includes E2EE sync and encrypted cloud backup. Cancel anytime in your device\'s subscription settings.'**
  String get billingSubscriptionBody;

  /// No description provided for @billingStatusFree.
  ///
  /// In en, this message translates to:
  /// **'Free'**
  String get billingStatusFree;

  /// No description provided for @billingStatusTrial.
  ///
  /// In en, this message translates to:
  /// **'Trial'**
  String get billingStatusTrial;

  /// No description provided for @billingStatusActive.
  ///
  /// In en, this message translates to:
  /// **'Active'**
  String get billingStatusActive;

  /// No description provided for @billingStatusGrace.
  ///
  /// In en, this message translates to:
  /// **'Payment grace period'**
  String get billingStatusGrace;

  /// No description provided for @billingStatusExpired.
  ///
  /// In en, this message translates to:
  /// **'Expired'**
  String get billingStatusExpired;

  /// No description provided for @billingStatusRevoked.
  ///
  /// In en, this message translates to:
  /// **'Revoked'**
  String get billingStatusRevoked;

  /// No description provided for @billingMonthlyLabel.
  ///
  /// In en, this message translates to:
  /// **'Monthly'**
  String get billingMonthlyLabel;

  /// No description provided for @billingYearlyLabel.
  ///
  /// In en, this message translates to:
  /// **'Yearly'**
  String get billingYearlyLabel;

  /// No description provided for @billingPurchaseButton.
  ///
  /// In en, this message translates to:
  /// **'Start Pro'**
  String get billingPurchaseButton;

  /// No description provided for @billingRestoreButton.
  ///
  /// In en, this message translates to:
  /// **'Restore purchases'**
  String get billingRestoreButton;

  /// No description provided for @billingManageButton.
  ///
  /// In en, this message translates to:
  /// **'Manage subscription'**
  String get billingManageButton;

  /// No description provided for @billingUnavailable.
  ///
  /// In en, this message translates to:
  /// **'Billing is unavailable right now.'**
  String get billingUnavailable;

  /// No description provided for @billingCancelled.
  ///
  /// In en, this message translates to:
  /// **'Purchase cancelled.'**
  String get billingCancelled;

  /// No description provided for @billingPending.
  ///
  /// In en, this message translates to:
  /// **'Purchase pending. Pro will activate after your app store confirms payment.'**
  String get billingPending;

  /// No description provided for @billingFailed.
  ///
  /// In en, this message translates to:
  /// **'Purchase couldn\'t be completed.'**
  String get billingFailed;

  /// No description provided for @billingRestored.
  ///
  /// In en, this message translates to:
  /// **'Purchase status refreshed.'**
  String get billingRestored;

  /// No description provided for @billingPriceSemantics.
  ///
  /// In en, this message translates to:
  /// **'{period}, {price}'**
  String billingPriceSemantics(String period, String price);

  /// No description provided for @backButtonTooltip.
  ///
  /// In en, this message translates to:
  /// **'Back'**
  String get backButtonTooltip;

  /// No description provided for @templatesTitle.
  ///
  /// In en, this message translates to:
  /// **'Templates'**
  String get templatesTitle;

  /// No description provided for @templatesLoadFailed.
  ///
  /// In en, this message translates to:
  /// **'Templates could not be loaded: {error}'**
  String templatesLoadFailed(String error);

  /// No description provided for @templatesEmptyTitle.
  ///
  /// In en, this message translates to:
  /// **'No templates yet'**
  String get templatesEmptyTitle;

  /// No description provided for @templatesEmptyBody.
  ///
  /// In en, this message translates to:
  /// **'Create a reusable task blueprint here, or save an existing task as a template.'**
  String get templatesEmptyBody;

  /// No description provided for @newTemplateButton.
  ///
  /// In en, this message translates to:
  /// **'New template'**
  String get newTemplateButton;

  /// No description provided for @newTemplateTitle.
  ///
  /// In en, this message translates to:
  /// **'New template'**
  String get newTemplateTitle;

  /// No description provided for @templateActionsTooltip.
  ///
  /// In en, this message translates to:
  /// **'Template actions'**
  String get templateActionsTooltip;

  /// No description provided for @editButton.
  ///
  /// In en, this message translates to:
  /// **'Edit'**
  String get editButton;

  /// No description provided for @replaceTemplateSnapshotMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Replace contents'**
  String get replaceTemplateSnapshotMenuItem;

  /// No description provided for @templateTaskCount.
  ///
  /// In en, this message translates to:
  /// **'{count, plural, =1{1 task} other{{count} tasks}}'**
  String templateTaskCount(int count);

  /// No description provided for @createFromTemplateButton.
  ///
  /// In en, this message translates to:
  /// **'Create tasks'**
  String get createFromTemplateButton;

  /// No description provided for @addScheduleTooltip.
  ///
  /// In en, this message translates to:
  /// **'Add recurring task'**
  String get addScheduleTooltip;

  /// No description provided for @createTaskSeriesTooltip.
  ///
  /// In en, this message translates to:
  /// **'Create recurring tasks from this template'**
  String get createTaskSeriesTooltip;

  /// No description provided for @taskSeriesTitle.
  ///
  /// In en, this message translates to:
  /// **'Recurring tasks'**
  String get taskSeriesTitle;

  /// No description provided for @taskSeriesEmptyTitle.
  ///
  /// In en, this message translates to:
  /// **'No recurring tasks'**
  String get taskSeriesEmptyTitle;

  /// No description provided for @taskSeriesEmptyBody.
  ///
  /// In en, this message translates to:
  /// **'Create one from a template. It becomes independent after creation.'**
  String get taskSeriesEmptyBody;

  /// No description provided for @templateCreatedMessage.
  ///
  /// In en, this message translates to:
  /// **'Tasks created from template.'**
  String get templateCreatedMessage;

  /// No description provided for @replaceTemplateSnapshotTitle.
  ///
  /// In en, this message translates to:
  /// **'Replace template contents'**
  String get replaceTemplateSnapshotTitle;

  /// No description provided for @duplicateTemplateMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Duplicate'**
  String get duplicateTemplateMenuItem;

  /// No description provided for @templateCopyName.
  ///
  /// In en, this message translates to:
  /// **'{name} copy'**
  String templateCopyName(String name);

  /// No description provided for @sourceTaskIdLabel.
  ///
  /// In en, this message translates to:
  /// **'Source task ID'**
  String get sourceTaskIdLabel;

  /// No description provided for @replaceButton.
  ///
  /// In en, this message translates to:
  /// **'Replace'**
  String get replaceButton;

  /// No description provided for @deleteTemplateDialogTitle.
  ///
  /// In en, this message translates to:
  /// **'Delete {name}?'**
  String deleteTemplateDialogTitle(String name);

  /// No description provided for @deleteTemplateDialogBody.
  ///
  /// In en, this message translates to:
  /// **'Recurring tasks and tasks created from this template will stay unchanged.'**
  String get deleteTemplateDialogBody;

  /// No description provided for @deleteScheduleDialogTitle.
  ///
  /// In en, this message translates to:
  /// **'Delete recurring task?'**
  String get deleteScheduleDialogTitle;

  /// No description provided for @deleteScheduleDialogBody.
  ///
  /// In en, this message translates to:
  /// **'Tasks already created will stay unchanged.'**
  String get deleteScheduleDialogBody;

  /// No description provided for @scheduleEndedLabel.
  ///
  /// In en, this message translates to:
  /// **'Ended'**
  String get scheduleEndedLabel;

  /// No description provided for @scheduleSemantics.
  ///
  /// In en, this message translates to:
  /// **'Recurring task {rule}, next {next}'**
  String scheduleSemantics(String rule, String next);

  /// No description provided for @scheduleStreak.
  ///
  /// In en, this message translates to:
  /// **'{count} streak'**
  String scheduleStreak(int count);

  /// No description provided for @scheduleActionsTooltip.
  ///
  /// In en, this message translates to:
  /// **'Recurring task actions'**
  String get scheduleActionsTooltip;

  /// No description provided for @pauseScheduleMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Pause'**
  String get pauseScheduleMenuItem;

  /// No description provided for @resumeScheduleMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Resume'**
  String get resumeScheduleMenuItem;

  /// No description provided for @editTemplateTitle.
  ///
  /// In en, this message translates to:
  /// **'Edit template'**
  String get editTemplateTitle;

  /// No description provided for @blueprintRootLabel.
  ///
  /// In en, this message translates to:
  /// **'Root task'**
  String get blueprintRootLabel;

  /// No description provided for @blueprintChildLabel.
  ///
  /// In en, this message translates to:
  /// **'Child task'**
  String get blueprintChildLabel;

  /// No description provided for @addBlueprintChildButton.
  ///
  /// In en, this message translates to:
  /// **'Add child task'**
  String get addBlueprintChildButton;

  /// No description provided for @moveUpButton.
  ///
  /// In en, this message translates to:
  /// **'Move up'**
  String get moveUpButton;

  /// No description provided for @moveDownButton.
  ///
  /// In en, this message translates to:
  /// **'Move down'**
  String get moveDownButton;

  /// No description provided for @defaultListLabel.
  ///
  /// In en, this message translates to:
  /// **'Default list'**
  String get defaultListLabel;

  /// No description provided for @inboxFallbackLabel.
  ///
  /// In en, this message translates to:
  /// **'Inbox fallback'**
  String get inboxFallbackLabel;

  /// No description provided for @newScheduleTitle.
  ///
  /// In en, this message translates to:
  /// **'New recurring task'**
  String get newScheduleTitle;

  /// No description provided for @editScheduleTitle.
  ///
  /// In en, this message translates to:
  /// **'Edit recurring task'**
  String get editScheduleTitle;

  /// No description provided for @schedulePresetLabel.
  ///
  /// In en, this message translates to:
  /// **'Repeat'**
  String get schedulePresetLabel;

  /// No description provided for @dailyPreset.
  ///
  /// In en, this message translates to:
  /// **'Every day'**
  String get dailyPreset;

  /// No description provided for @weeklyPreset.
  ///
  /// In en, this message translates to:
  /// **'Every week on this weekday'**
  String get weeklyPreset;

  /// No description provided for @monthlyPreset.
  ///
  /// In en, this message translates to:
  /// **'Every month on this date'**
  String get monthlyPreset;

  /// No description provided for @advancedPreset.
  ///
  /// In en, this message translates to:
  /// **'Advanced RRULE'**
  String get advancedPreset;

  /// No description provided for @rruleLabel.
  ///
  /// In en, this message translates to:
  /// **'RRULE'**
  String get rruleLabel;

  /// No description provided for @scheduleStartsAtLabel.
  ///
  /// In en, this message translates to:
  /// **'Starts'**
  String get scheduleStartsAtLabel;

  /// No description provided for @timeZoneLabel.
  ///
  /// In en, this message translates to:
  /// **'Time zone'**
  String get timeZoneLabel;

  /// No description provided for @scheduleEnabledLabel.
  ///
  /// In en, this message translates to:
  /// **'Recurring task enabled'**
  String get scheduleEnabledLabel;

  /// No description provided for @editSeriesContentMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Edit tasks and destination'**
  String get editSeriesContentMenuItem;

  /// No description provided for @editSeriesContentTitle.
  ///
  /// In en, this message translates to:
  /// **'Edit recurring task contents'**
  String get editSeriesContentTitle;

  /// No description provided for @targetListLabel.
  ///
  /// In en, this message translates to:
  /// **'Destination list'**
  String get targetListLabel;

  /// No description provided for @scheduleValidationFailed.
  ///
  /// In en, this message translates to:
  /// **'Check the recurrence rule, start, and time zone.'**
  String get scheduleValidationFailed;

  /// No description provided for @saveAsTemplateMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Save as template'**
  String get saveAsTemplateMenuItem;

  /// No description provided for @createTaskSeriesMenuItem.
  ///
  /// In en, this message translates to:
  /// **'Make recurring'**
  String get createTaskSeriesMenuItem;

  /// No description provided for @taskSeriesCreatedMessage.
  ///
  /// In en, this message translates to:
  /// **'Recurring task created.'**
  String get taskSeriesCreatedMessage;

  /// No description provided for @saveAsTemplateTitle.
  ///
  /// In en, this message translates to:
  /// **'Save as template'**
  String get saveAsTemplateTitle;

  /// No description provided for @templateSavedMessage.
  ///
  /// In en, this message translates to:
  /// **'Template saved.'**
  String get templateSavedMessage;

  /// No description provided for @bridgeErrorInvalidInput.
  ///
  /// In en, this message translates to:
  /// **'Check the information you entered.'**
  String get bridgeErrorInvalidInput;

  /// No description provided for @bridgeErrorNotFound.
  ///
  /// In en, this message translates to:
  /// **'That item is no longer available.'**
  String get bridgeErrorNotFound;

  /// No description provided for @bridgeErrorConflict.
  ///
  /// In en, this message translates to:
  /// **'The item changed. Reload it and try again.'**
  String get bridgeErrorConflict;

  /// No description provided for @bridgeErrorUnauthorized.
  ///
  /// In en, this message translates to:
  /// **'Sign in again to continue.'**
  String get bridgeErrorUnauthorized;

  /// No description provided for @bridgeErrorCredentialUnavailable.
  ///
  /// In en, this message translates to:
  /// **'Your sign-in credentials are unavailable. Sign in again.'**
  String get bridgeErrorCredentialUnavailable;

  /// No description provided for @bridgeErrorAccountBoundUnavailable.
  ///
  /// In en, this message translates to:
  /// **'This device cannot access the account encryption keys. Sign in again.'**
  String get bridgeErrorAccountBoundUnavailable;

  /// No description provided for @bridgeErrorEntitlementRequired.
  ///
  /// In en, this message translates to:
  /// **'This action requires a Pro subscription.'**
  String get bridgeErrorEntitlementRequired;

  /// No description provided for @bridgeErrorUpgradeRequired.
  ///
  /// In en, this message translates to:
  /// **'Update Taskveil before syncing again.'**
  String get bridgeErrorUpgradeRequired;

  /// No description provided for @bridgeErrorBusy.
  ///
  /// In en, this message translates to:
  /// **'Taskveil is busy with another operation. Try again shortly.'**
  String get bridgeErrorBusy;

  /// No description provided for @bridgeErrorLeaseLost.
  ///
  /// In en, this message translates to:
  /// **'Sync ownership changed. Try syncing again.'**
  String get bridgeErrorLeaseLost;

  /// No description provided for @bridgeErrorClockSkew.
  ///
  /// In en, this message translates to:
  /// **'This device\'s clock is ahead. Check the time and try again.'**
  String get bridgeErrorClockSkew;

  /// No description provided for @bridgeErrorCryptoUnavailable.
  ///
  /// In en, this message translates to:
  /// **'Secure local data is unavailable on this device.'**
  String get bridgeErrorCryptoUnavailable;

  /// No description provided for @bridgeErrorStorageFailure.
  ///
  /// In en, this message translates to:
  /// **'Taskveil could not access its local data.'**
  String get bridgeErrorStorageFailure;

  /// No description provided for @bridgeErrorSyncFailure.
  ///
  /// In en, this message translates to:
  /// **'Taskveil could not sync. Try again.'**
  String get bridgeErrorSyncFailure;

  /// No description provided for @bridgeErrorInternal.
  ///
  /// In en, this message translates to:
  /// **'Taskveil could not complete the operation.'**
  String get bridgeErrorInternal;

  /// No description provided for @failedToStartTaskveil.
  ///
  /// In en, this message translates to:
  /// **'Failed to start Taskveil: {error}'**
  String failedToStartTaskveil(String error);
}

class _AppLocalizationsDelegate
    extends LocalizationsDelegate<AppLocalizations> {
  const _AppLocalizationsDelegate();

  @override
  Future<AppLocalizations> load(Locale locale) {
    return SynchronousFuture<AppLocalizations>(lookupAppLocalizations(locale));
  }

  @override
  bool isSupported(Locale locale) =>
      <String>['en', 'ja'].contains(locale.languageCode);

  @override
  bool shouldReload(_AppLocalizationsDelegate old) => false;
}

AppLocalizations lookupAppLocalizations(Locale locale) {
  // Lookup logic when only language code is specified.
  switch (locale.languageCode) {
    case 'en':
      return AppLocalizationsEn();
    case 'ja':
      return AppLocalizationsJa();
  }

  throw FlutterError(
    'AppLocalizations.delegate failed to load unsupported locale "$locale". This is likely '
    'an issue with the localizations generation tool. Please file an issue '
    'on GitHub with a reproducible sample app and the gen-l10n configuration '
    'that was used.',
  );
}
