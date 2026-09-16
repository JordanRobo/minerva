// @generated automatically by Diesel CLI.

diesel::table! {
    goal_milestones (goal_id, milestone_id) {
        goal_id -> Uuid,
        milestone_id -> Uuid,
    }
}

diesel::table! {
    goals (id) {
        id -> Uuid,
        title -> Text,
        description -> Nullable<Text>,
        status -> Text,
        status_source -> Text,
        target_date -> Nullable<Date>,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    milestones (id) {
        id -> Uuid,
        title -> Text,
        description -> Nullable<Text>,
        status -> Text,
        status_source -> Text,
        target_date -> Nullable<Date>,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    progress_snapshots (id) {
        id -> Uuid,
        goal_id -> Nullable<Uuid>,
        milestone_id -> Nullable<Uuid>,
        recorded_at -> Timestamptz,
        status -> Text,
        percent_complete -> Int2,
        note -> Nullable<Text>,
    }
}

diesel::table! {
    tasks (id) {
        id -> Uuid,
        milestone_id -> Nullable<Uuid>,
        title -> Text,
        description -> Nullable<Text>,
        status -> Text,
        target_date -> Nullable<Date>,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::joinable!(goal_milestones -> goals (goal_id));
diesel::joinable!(goal_milestones -> milestones (milestone_id));
diesel::joinable!(progress_snapshots -> goals (goal_id));
diesel::joinable!(progress_snapshots -> milestones (milestone_id));
diesel::joinable!(tasks -> milestones (milestone_id));

diesel::allow_tables_to_appear_in_same_query!(
    goal_milestones,
    goals,
    milestones,
    progress_snapshots,
    tasks,
);
