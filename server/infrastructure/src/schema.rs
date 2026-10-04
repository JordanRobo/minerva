// @generated automatically by Diesel CLI.

diesel::table! {
    account_tokens (id) {
        id -> Uuid,
        purpose -> Text,
        token_hash -> Text,
        email -> Nullable<Text>,
        role -> Nullable<Text>,
        user_id -> Nullable<Uuid>,
        created_by -> Nullable<Uuid>,
        created_at -> Timestamptz,
        expires_at -> Timestamptz,
        consumed_at -> Nullable<Timestamptz>,
        revoked_at -> Nullable<Timestamptz>,
    }
}

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
    sessions (id) {
        id -> Uuid,
        user_id -> Uuid,
        token_hash -> Text,
        created_at -> Timestamptz,
        expires_at -> Timestamptz,
        last_seen_at -> Timestamptz,
    }
}

diesel::table! {
    sso_group_role_rules (id) {
        id -> Uuid,
        group_name -> Text,
        role -> Text,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    task_relations (id) {
        id -> Uuid,
        source_task_id -> Uuid,
        target_task_id -> Uuid,
        relation_type -> Text,
        created_at -> Timestamptz,
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

diesel::table! {
    user_identities (id) {
        id -> Uuid,
        user_id -> Uuid,
        issuer -> Text,
        subject -> Text,
        email -> Nullable<Text>,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    users (id) {
        id -> Uuid,
        email -> Text,
        password_hash -> Nullable<Text>,
        display_name -> Text,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
        role -> Text,
        deactivated_at -> Nullable<Timestamptz>,
    }
}

// `account_tokens` has two foreign keys to `users` (`user_id` and
// `created_by`), so `print-schema` omits the pair; the default join is on
// `user_id`. See the note in `repositories/mod.rs`.
diesel::joinable!(account_tokens -> users (user_id));
diesel::joinable!(goal_milestones -> goals (goal_id));
diesel::joinable!(goal_milestones -> milestones (milestone_id));
diesel::joinable!(progress_snapshots -> goals (goal_id));
diesel::joinable!(progress_snapshots -> milestones (milestone_id));
diesel::joinable!(sessions -> users (user_id));
diesel::joinable!(tasks -> milestones (milestone_id));
diesel::joinable!(user_identities -> users (user_id));

diesel::allow_tables_to_appear_in_same_query!(
    account_tokens,
    goal_milestones,
    goals,
    milestones,
    progress_snapshots,
    sessions,
    sso_group_role_rules,
    task_relations,
    tasks,
    user_identities,
    users,
);
