//! src/garmin/health_fields.rs declares the supported Garmin-to-domain field mappings.
//! Each allowlisted scalar has a stable name and unit; identifiers and raw payloads are excluded.

pub(super) type Field = (&'static str, &'static str, &'static str);
pub(super) const DAILY: &[Field] = &[
    ("totalSteps", "steps", "steps"),
    ("dailyStepGoal", "step_goal", "steps"),
    ("totalDistanceMeters", "distance", "m"),
    ("floorsAscended", "floors_ascended", "floors"),
    ("floorsDescended", "floors_descended", "floors"),
    ("totalKilocalories", "calories_total", "kcal"),
    ("activeKilocalories", "calories_active", "kcal"),
    ("bmrKilocalories", "calories_resting", "kcal"),
    ("moderateIntensityMinutes", "intensity_moderate", "min"),
    ("vigorousIntensityMinutes", "intensity_vigorous", "min"),
    ("restingHeartRate", "heart_rate_resting", "bpm"),
    ("minHeartRate", "heart_rate_min", "bpm"),
    ("maxHeartRate", "heart_rate_max", "bpm"),
    (
        "lastSevenDaysAvgRestingHeartRate",
        "heart_rate_resting_7d",
        "bpm",
    ),
    ("averageStressLevel", "stress_average", "score"),
    ("maxStressLevel", "stress_max", "score"),
    ("stressDuration", "stress_duration", "s"),
    ("restStressDuration", "stress_rest_duration", "s"),
    ("lowStressDuration", "stress_low_duration", "s"),
    ("mediumStressDuration", "stress_medium_duration", "s"),
    ("highStressDuration", "stress_high_duration", "s"),
    ("bodyBatteryHighestValue", "body_battery_max", "score"),
    ("bodyBatteryLowestValue", "body_battery_min", "score"),
    ("bodyBatteryMostRecentValue", "body_battery_latest", "score"),
    ("bodyBatteryChargedValue", "body_battery_charged", "points"),
    ("bodyBatteryDrainedValue", "body_battery_drained", "points"),
    ("bodyBatteryAtWakeTime", "body_battery_at_wake", "score"),
    ("hydrationValueInML", "hydration", "ml"),
    ("hydrationGoalInML", "hydration_goal", "ml"),
    (
        "avgWakingRespirationValue",
        "respiration_average_awake",
        "breaths/min",
    ),
    ("highestRespirationValue", "respiration_max", "breaths/min"),
    ("lowestRespirationValue", "respiration_min", "breaths/min"),
    (
        "latestRespirationValue",
        "respiration_latest",
        "breaths/min",
    ),
    ("averageSpo2", "spo2_average", "%"),
];
pub(super) const SLEEP: &[Field] = &[
    ("sleepTimeSeconds", "duration", "s"),
    ("deepSleepSeconds", "deep", "s"),
    ("lightSleepSeconds", "light", "s"),
    ("remSleepSeconds", "rem", "s"),
    ("awakeSleepSeconds", "awake", "s"),
    ("napTimeSeconds", "naps", "s"),
    ("awakeCount", "awakenings", "count"),
    ("avgSleepStress", "stress_average", "score"),
    ("avgHeartRate", "heart_rate_average", "bpm"),
    ("averageSpO2Value", "spo2_average", "%"),
    (
        "averageRespirationValue",
        "respiration_average",
        "breaths/min",
    ),
    ("/sleepScores/overall/value", "score", "score"),
    ("/sleepScores/overall/qualifierKey", "quality", "text"),
    ("sleepScoreFeedback", "feedback", "text"),
];
pub(super) const HRV: &[Field] = &[
    ("lastNightAvg", "overnight_average", "ms"),
    ("lastNight5MinHigh", "overnight_5min_high", "ms"),
    ("weeklyAvg", "weekly_average", "ms"),
    ("status", "status", "text"),
    ("/baseline/balancedLow", "baseline_low", "ms"),
    ("/baseline/balancedUpper", "baseline_high", "ms"),
];
pub(super) const HEART_RATE: &[Field] = &[
    ("restingHeartRate", "resting", "bpm"),
    ("minHeartRate", "min", "bpm"),
    ("maxHeartRate", "max", "bpm"),
    ("lastSevenDaysAvgRestingHeartRate", "resting_7d", "bpm"),
];
pub(super) const BODY_BATTERY: &[Field] = &[
    ("charged", "charged", "points"),
    ("drained", "drained", "points"),
];
pub(super) const SPO2: &[Field] = &[
    ("averageSpO2", "average", "%"),
    ("lowestSpO2", "lowest", "%"),
    ("latestSpO2", "latest", "%"),
    ("lastSevenDaysAvgSpO2", "average_7d", "%"),
    ("avgSleepSpO2", "sleep_average", "%"),
];
// The weight service reports body mass in grams; the decoder stores kilograms.
pub(super) const WEIGHT: &[Field] = &[
    ("weight", "weight", "g"),
    ("bmi", "bmi", "kg/m2"),
    ("bodyFat", "body_fat", "%"),
    ("bodyWater", "body_water", "%"),
    ("metabolicAge", "metabolic_age", "years"),
];
pub(super) const READINESS: &[Field] = &[
    ("score", "score", "score"),
    ("level", "level", "text"),
    ("inputContext", "input_context", "text"),
    ("feedbackLong", "feedback_long", "text"),
    ("feedbackShort", "feedback_short", "text"),
    ("sleepScore", "sleep_score", "score"),
    ("sleepScoreFactorPercent", "sleep_factor", "%"),
    ("sleepScoreFactorFeedback", "sleep_feedback", "text"),
    ("recoveryTime", "recovery_time", "min"),
    ("recoveryTimeChangePhrase", "recovery_time_change", "text"),
    ("recoveryTimeFactorPercent", "recovery_factor", "%"),
    ("recoveryTimeFactorFeedback", "recovery_feedback", "text"),
    ("acwrFactorPercent", "load_factor", "%"),
    ("acwrFactorFeedback", "load_feedback", "text"),
    ("hrvFactorPercent", "hrv_factor", "%"),
    ("hrvFactorFeedback", "hrv_feedback", "text"),
    ("stressHistoryFactorPercent", "stress_history_factor", "%"),
    (
        "stressHistoryFactorFeedback",
        "stress_history_feedback",
        "text",
    ),
    ("sleepHistoryFactorPercent", "sleep_history_factor", "%"),
    (
        "sleepHistoryFactorFeedback",
        "sleep_history_feedback",
        "text",
    ),
];
