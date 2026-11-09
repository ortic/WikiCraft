-- Webhook payload format: WikiCraft JSON, or a chat message for Slack/Mattermost, Microsoft
-- Teams (Workflows: Adaptive Card) or Google Chat incoming webhooks.
ALTER TABLE webhooks ADD COLUMN format text NOT NULL DEFAULT 'json';
