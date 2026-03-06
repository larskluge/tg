# Complete Telegram Client API Implementation Spec

## Overview

This document is a full implementation spec for the `tg` CLI, mapping every TDLib client API capability (non-Bot API) against what is currently implemented. The goal is to reach full coverage of Telegram's client API surface.

**Current state:** 9 CLI commands, 18 TDLib functions used, 13 message types handled.
**Target state:** ~50+ CLI commands, 200+ TDLib functions used, all message/chat/media types handled.

---

## Table of Contents

1. [Currently Implemented](#1-currently-implemented)
2. [Authentication & Sessions](#2-authentication--sessions)
3. [Chats & Chat Lists](#3-chats--chat-lists)
4. [Messages](#4-messages)
5. [Users & Contacts](#5-users--contacts)
6. [Groups & Supergroups](#6-groups--supergroups)
7. [Channels](#7-channels)
8. [Media & Files](#8-media--files)
9. [Stickers & Custom Emoji](#9-stickers--custom-emoji)
10. [Reactions](#10-reactions)
11. [Polls](#11-polls)
12. [Search](#12-search)
13. [Notifications](#13-notifications)
14. [Chat Folders](#14-chat-folders)
15. [Forum Topics](#15-forum-topics)
16. [Stories](#16-stories)
17. [Calls](#17-calls)
18. [Secret Chats](#18-secret-chats)
19. [Scheduled Messages](#19-scheduled-messages)
20. [Saved Messages](#20-saved-messages)
21. [Account & Settings](#21-account--settings)
22. [Privacy & Security](#22-privacy--security)
23. [Contacts Management](#23-contacts-management)
24. [Inline Mode](#24-inline-mode)
25. [Payments & Stars](#25-payments--stars)
26. [Premium](#26-premium)
27. [Backgrounds & Themes](#27-backgrounds--themes)
28. [Language Packs](#28-language-packs)
29. [Proxy & Network](#29-proxy--network)
30. [Statistics & Analytics](#30-statistics--analytics)
31. [Deep Links](#31-deep-links)
32. [Web Apps (Mini Apps)](#32-web-apps-mini-apps)
33. [Sponsored Messages & Ads](#33-sponsored-messages--ads)
34. [Gifts](#34-gifts)
35. [Passport](#35-passport)
36. [Quick Reply Shortcuts](#36-quick-reply-shortcuts)
37. [Reporting](#37-reporting)
38. [Logging & Debug](#38-logging--debug)
39. [Implementation Priority](#39-implementation-priority)

---

## 1. Currently Implemented

### CLI Commands (9)

| Command | Description | TDLib Functions Used |
|---------|-------------|---------------------|
| `auth --phone <PHONE>` | Authenticate with phone/code/2FA | `set_tdlib_parameters`, `set_authentication_phone_number`, `check_authentication_code`, `check_authentication_password`, `get_authorization_state` |
| `chats [--limit N]` | List 1:1 private chats | `get_chats`, `get_chat` |
| `groups [--limit N]` | List group chats | `get_chats`, `get_chat` |
| `unread [--limit N]` | List unread chats | `get_chats`, `get_chat` |
| `messages <NAME\|--chat ID> [--limit N]` | Read messages | `get_chat_history`, `get_user`, `get_chat` |
| `send <NAME\|--id ID\|--group GROUP> -m "MSG"` | Send text message | `send_message`, `search_contacts`, `create_private_chat`, `search_public_chats`, `get_chat` |
| `download --chat ID --message ID` | Download media | `get_message`, `download_file` |
| `search <QUERY>` | Search contacts | `search_contacts` |
| `mark-read <NAME\|--id ID>` | Mark chat as read | `view_messages`, `get_chat` |
| `mark-unread --id ID` | Mark chat as unread | `toggle_chat_is_marked_as_unread` |

### Message Types Handled (13)

Text, Photo, Video, Document, Sticker, Audio, VoiceNote, Animation, VideoNote, Location, Contact, AnimatedEmoji, Poll

### Auth States Handled (6)

WaitTdlibParameters, WaitPhoneNumber, WaitCode, WaitPassword, Ready, Closed

### TelegramClient Trait Methods (13)

`authenticate`, `is_authenticated`, `get_chats`, `get_groups`, `get_unread_chats`, `search_contacts`, `find_chat_by_name`, `find_group_by_name`, `send_message`, `get_messages`, `download_message_media`, `mark_chat_as_read`, `mark_chat_as_unread`

---

## 2. Authentication & Sessions

### Currently Implemented
- Phone number auth with code + 2FA password
- Session persistence in `dirs::data_dir()/tg`
- API credential storage in `credentials.json`

### Missing - To Implement

#### QR Code Login
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| QR code auth | `requestQrCodeAuthentication(other_user_ids)` | `tg auth --qr` — display QR link in terminal, poll for scan confirmation |
| Confirm QR on other device | `confirmQrCodeAuthentication(link)` | Used when THIS device scans another device's QR |

#### Session Management
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| List active sessions | `getActiveSessions()` → `Sessions` | `tg sessions` |
| Terminate a session | `terminateSession(session_id)` | `tg sessions --terminate <ID>` |
| Terminate all other sessions | `terminateAllOtherSessions()` | `tg sessions --terminate-all` |
| Confirm new session | `confirmSession(session_id)` | `tg sessions --confirm <ID>` |
| Set inactive session TTL | `setInactiveSessionTtl(inactive_session_ttl_days)` | `tg sessions --ttl <DAYS>` |
| Get inactive session TTL | `getInactiveSessionTtl()` | `tg sessions --ttl` (no value = show current) |

#### Connected Websites
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| List connected websites | `getConnectedWebsites()` → `ConnectedWebsites` | `tg websites` |
| Disconnect a website | `disconnectWebsite(website_id)` | `tg websites --disconnect <ID>` |
| Disconnect all websites | `disconnectAllWebsites()` | `tg websites --disconnect-all` |

#### Auth State Registration
| Feature | TDLib Functions | Notes |
|---------|----------------|-------|
| Register new account | `registerUser(first_name, last_name, disable_notification)` | Handle `authorizationStateWaitRegistration` |
| Email auth | `setAuthenticationEmailAddress(email)` | Handle `authorizationStateWaitEmailAddress` |
| Email code | `checkAuthenticationEmailCode(code)` | Handle `authorizationStateWaitEmailCode` |

#### Password Management
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get password state | `getPasswordState()` → `PasswordState` | `tg password --status` |
| Set new password | `setPassword(old_password, new_password, new_hint, set_recovery_email, new_recovery_email)` | `tg password --set` |
| Remove password | `setPassword(old_password, "", "", false, "")` | `tg password --remove` |
| Set recovery email | `setRecoveryEmailAddress(password, new_recovery_email)` | `tg password --recovery-email <EMAIL>` |
| Check recovery email code | `checkRecoveryEmailAddressCode(code)` | Interactive prompt |
| Recover password | `recoverPassword(recovery_code, new_password, new_hint)` | `tg password --recover` |
| Request recovery code | `requestPasswordRecovery()` | `tg password --request-recovery` |
| Reset password | `resetPassword()` | `tg password --reset` |

---

## 3. Chats & Chat Lists

### Currently Implemented
- List private chats, groups, unread chats
- Basic chat info (id, name, unread_count, last_message)

### Missing - To Implement

#### Chat List Management
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Load chat list | `loadChats(chat_list, limit)` | Internal (replace current `get_chats`) |
| Get all chats (unified) | `getChats(chat_list, limit)` | `tg chats --all` |
| Archive chat list | `getChats(chatListArchive, limit)` | `tg chats --archived` |
| Get chat | `getChat(chat_id)` | `tg chat <ID>` (show details) |

#### Chat Actions
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Open chat | `openChat(chat_id)` | Internal (before reading messages) |
| Close chat | `closeChat(chat_id)` | Internal (after reading messages) |
| Delete chat | `deleteChatHistory(chat_id, remove_from_chat_list, revoke)` | `tg chat delete <ID> [--revoke]` |
| Archive chat | `addChatToList(chat_id, chatListArchive)` | `tg chat archive <ID>` |
| Unarchive chat | `addChatToList(chat_id, chatListMain)` | `tg chat unarchive <ID>` |
| Pin chat | `toggleChatIsPinned(chat_list, chat_id, is_pinned)` | `tg chat pin <ID>` |
| Unpin chat | `toggleChatIsPinned(chat_list, chat_id, false)` | `tg chat unpin <ID>` |
| Clear chat history | `deleteChatHistory(chat_id, false, revoke)` | `tg chat clear <ID> [--revoke]` |

#### Chat Settings
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Set chat title | `setChatTitle(chat_id, title)` | `tg chat set-title <ID> "New Title"` |
| Set chat photo | `setChatPhoto(chat_id, photo)` | `tg chat set-photo <ID> <FILE>` |
| Delete chat photo | `deleteChatPhoto(chat_id)` | `tg chat delete-photo <ID>` |
| Set chat description | `setChatDescription(chat_id, description)` | `tg chat set-description <ID> "desc"` |
| Set chat draft | `setChatDraftMessage(chat_id, message_thread_id, draft)` | `tg chat set-draft <ID> "draft text"` |
| Mute chat | `setChatNotificationSettings(chat_id, settings)` | `tg chat mute <ID> [--duration <SECONDS>]` |
| Unmute chat | `setChatNotificationSettings(chat_id, default_settings)` | `tg chat unmute <ID>` |
| Set chat message TTL | `setChatMessageAutoDeleteTime(chat_id, message_auto_delete_time)` | `tg chat set-ttl <ID> <SECONDS>` |
| Block chat | `setChatBlockList(chat_id, blockListMain)` | `tg chat block <ID>` |
| Unblock chat | `setChatBlockList(chat_id, null)` | `tg chat unblock <ID>` |

#### Chat Info Queries
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get chat member count | `getChatMemberCount(chat_id)` | Shown in `tg chat <ID>` |
| Get chat admins | `getChatAdministrators(chat_id)` → `ChatAdministrators` | `tg chat admins <ID>` |
| Get chat member | `getChatMember(chat_id, member_id)` | `tg chat member <ID> <USER_ID>` |
| Search chat members | `searchChatMembers(chat_id, query, limit, filter)` | `tg chat members <ID> [--query Q] [--filter admins\|banned\|bots]` |
| Get chat invite link | `getInviteLink(chat_id)` | `tg chat invite-link <ID>` |
| Join chat by invite link | `joinChatByInviteLink(invite_link)` | `tg join <INVITE_LINK>` |
| Leave chat | `leaveChat(chat_id)` | `tg chat leave <ID>` |

#### Chat Invite Links
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Create invite link | `createChatInviteLink(chat_id, name, expiration_date, member_limit, creates_join_request)` | `tg invite create <CHAT_ID> [options]` |
| Edit invite link | `editChatInviteLink(...)` | `tg invite edit <CHAT_ID> <LINK> [options]` |
| Revoke invite link | `revokeChatInviteLink(chat_id, invite_link)` | `tg invite revoke <CHAT_ID> <LINK>` |
| Delete invite link | `deleteChatInviteLink(chat_id, invite_link)` | `tg invite delete <CHAT_ID> <LINK>` |
| Get invite links | `getChatInviteLinks(chat_id, creator_user_id, is_revoked, offset_date, offset_invite_link, limit)` | `tg invite list <CHAT_ID>` |
| Get invite link members | `getChatInviteLinkMembers(chat_id, invite_link, only_with_expired_subscription, offset_member, limit)` | `tg invite members <CHAT_ID> <LINK>` |
| Get join requests | `getChatJoinRequests(chat_id, invite_link, query, offset_request, limit)` | `tg invite requests <CHAT_ID>` |
| Process join request | `processChatJoinRequest(chat_id, user_id, approve)` | `tg invite approve/deny <CHAT_ID> <USER_ID>` |
| Process all join requests | `processChatJoinRequests(chat_id, invite_link, approve)` | `tg invite approve-all/deny-all <CHAT_ID>` |

---

## 4. Messages

### Currently Implemented
- Send text messages
- Read message history with pagination + retry
- Parse 13 message content types
- Message timestamps (RFC3339)

### Missing - To Implement

#### Message Sending (Extended)
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Send with formatting | `sendMessage(...)` with `formattedText(text, entities)` | `tg send <TARGET> -m "**bold** _italic_" --parse-mode markdown` |
| Send photo | `sendMessage(...)` with `inputMessagePhoto(photo, thumbnail, added_sticker_file_ids, width, height, caption, show_caption_above_media, self_destruct_type, has_spoiler)` | `tg send <TARGET> --photo <FILE> [-m "caption"]` |
| Send video | `sendMessage(...)` with `inputMessageVideo(video, thumbnail, added_sticker_file_ids, duration, width, height, supports_streaming, caption, show_caption_above_media, self_destruct_type, has_spoiler)` | `tg send <TARGET> --video <FILE> [-m "caption"]` |
| Send document | `sendMessage(...)` with `inputMessageDocument(document, thumbnail, disable_content_type_detection, caption)` | `tg send <TARGET> --file <FILE> [-m "caption"]` |
| Send audio | `sendMessage(...)` with `inputMessageAudio(audio, album_cover_thumbnail, duration, title, performer, caption)` | `tg send <TARGET> --audio <FILE> [-m "caption"]` |
| Send voice note | `sendMessage(...)` with `inputMessageVoiceNote(voice_note, duration, waveform, caption, self_destruct_type)` | `tg send <TARGET> --voice <FILE>` |
| Send video note | `sendMessage(...)` with `inputMessageVideoNote(video_note, thumbnail, duration, length, self_destruct_type)` | `tg send <TARGET> --video-note <FILE>` |
| Send location | `sendMessage(...)` with `inputMessageLocation(location, live_period, heading, proximity_alert_radius)` | `tg send <TARGET> --location <LAT,LON> [--live <SECONDS>]` |
| Send contact | `sendMessage(...)` with `inputMessageContact(contact)` | `tg send <TARGET> --contact <PHONE> <FIRST> [LAST]` |
| Send sticker | `sendMessage(...)` with `inputMessageSticker(sticker, thumbnail, width, height, emoji)` | `tg send <TARGET> --sticker <FILE\|ID>` |
| Send animation | `sendMessage(...)` with `inputMessageAnimation(...)` | `tg send <TARGET> --gif <FILE>` |
| Send album | `sendMessageAlbum(chat_id, message_thread_id, reply_to, options, input_message_contents)` | `tg send <TARGET> --album <FILE1> <FILE2> ...` |
| Reply to message | `sendMessage(...)` with `inputMessageReplyToMessage(message_id, quote)` | `tg send <TARGET> -m "text" --reply-to <MSG_ID>` |
| Forward message | `forwardMessages(chat_id, message_thread_id, from_chat_id, message_ids, options, send_copy, remove_caption)` | `tg forward <FROM_CHAT> <MSG_ID> <TO_CHAT>` |
| Send silently | `sendMessage(...)` with `messageSendOptions(disable_notification: true, ...)` | `tg send <TARGET> -m "text" --silent` |

#### Message Editing & Deletion
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Edit text | `editMessageText(chat_id, message_id, reply_markup, input_message_content)` | `tg edit <CHAT_ID> <MSG_ID> "new text"` |
| Edit caption | `editMessageCaption(chat_id, message_id, reply_markup, caption, show_caption_above_media)` | `tg edit-caption <CHAT_ID> <MSG_ID> "new caption"` |
| Edit media | `editMessageMedia(chat_id, message_id, reply_markup, input_message_content)` | `tg edit-media <CHAT_ID> <MSG_ID> <NEW_FILE>` |
| Delete messages | `deleteMessages(chat_id, message_ids, revoke)` | `tg delete <CHAT_ID> <MSG_IDS...> [--revoke]` |
| Delete chat history | `deleteChatHistory(chat_id, remove_from_chat_list, revoke)` | `tg chat clear <CHAT_ID>` |

#### Message Pinning
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Pin message | `pinChatMessage(chat_id, message_id, disable_notification, only_for_self)` | `tg pin <CHAT_ID> <MSG_ID> [--silent] [--only-self]` |
| Unpin message | `unpinChatMessage(chat_id, message_id)` | `tg unpin <CHAT_ID> <MSG_ID>` |
| Unpin all | `unpinAllChatMessages(chat_id)` | `tg unpin <CHAT_ID> --all` |

#### Message Threads
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get thread | `getMessageThread(chat_id, message_id)` → `MessageThreadInfo` | `tg thread <CHAT_ID> <MSG_ID>` |
| Get thread history | `getMessageThreadHistory(chat_id, message_id, from_message_id, offset, limit)` | `tg thread <CHAT_ID> <MSG_ID> --messages [--limit N]` |

#### Message Queries
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get message | `getMessage(chat_id, message_id)` | `tg message <CHAT_ID> <MSG_ID>` |
| Get messages | `getMessages(chat_id, message_ids)` | `tg message <CHAT_ID> <MSG_ID1> <MSG_ID2> ...` |
| Get message link | `getMessageLink(chat_id, message_id, media_timestamp, for_album, in_message_thread)` | `tg message link <CHAT_ID> <MSG_ID>` |
| Get replied message | `getRepliedMessage(chat_id, message_id)` | Shown in message display |
| Get message viewers | `getMessageViewers(chat_id, message_id)` | `tg message viewers <CHAT_ID> <MSG_ID>` |
| Get message read date | `getMessageReadDate(chat_id, message_id)` | Shown in message display |
| Translate text | `translateText(text, to_language_code)` | `tg translate "text" --lang <CODE>` |
| Translate message | `translateMessageText(chat_id, message_id, to_language_code)` | `tg translate <CHAT_ID> <MSG_ID> --lang <CODE>` |
| Recognize speech | `recognizeSpeech(chat_id, message_id)` | `tg transcribe <CHAT_ID> <MSG_ID>` |

#### Chat Actions (Typing indicators)
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Send typing | `sendChatAction(chat_id, message_thread_id, action)` | Internal (while composing) |

Action types: `chatActionTyping`, `chatActionRecordingVideo`, `chatActionUploadingPhoto`, `chatActionUploadingVideo`, `chatActionUploadingDocument`, `chatActionRecordingVoiceNote`, `chatActionUploadingVoiceNote`, `chatActionChoosingLocation`, `chatActionChoosingContact`, `chatActionChoosingSticker`, `chatActionRecordingVideoNote`, `chatActionUploadingVideoNote`, `chatActionWatchingAnimations`, `chatActionCancel`

#### Additional Message Content Types to Handle
| Type | TDLib Type | Display |
|------|-----------|---------|
| Venue | `messageVenue` | Location name + address + coordinates |
| Game | `messageGame` | Game title + description |
| Invoice | `messageInvoice` | Price + description |
| Dice | `messageDice` | Emoji + value |
| ProximityAlertTriggered | `messageProximityAlertTriggered` | User approached user within distance |
| VideoChatStarted | `messageVideoChatStarted` | "Video chat started" |
| VideoChatEnded | `messageVideoChatEnded` | "Video chat ended (duration)" |
| PinMessage | `messagePinMessage` | "Pinned message: ..." |
| ChatChangeTitle | `messageChatChangeTitle` | "Chat renamed to ..." |
| ChatAddMembers | `messageChatAddMembers` | "X added Y" |
| ChatDeleteMember | `messageChatDeleteMember` | "X removed Y" |
| ChatJoinByLink | `messageChatJoinByLink` | "X joined via invite link" |
| ScreenshotTaken | `messageScreenshotTaken` | "Screenshot taken" |
| GiftedPremium | `messageGiftedPremium` | "Gifted Premium for N months" |
| Story | `messageStory` | Story reference |
| Gift | `messageGift` | Gift details |
| PaidMedia | `messagePaidMedia` | Paid media details |

---

## 5. Users & Contacts

### Currently Implemented
- `get_user(user_id)` for message sender info
- `search_contacts(query, limit)` for contact search

### Missing - To Implement

#### User Info
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get user | `getUser(user_id)` → `User` | `tg user <ID>` |
| Get full user info | `getUserFullInfo(user_id)` → `UserFullInfo` | `tg user <ID> --full` |
| Get user profile photos | `getUserProfilePhotos(user_id, offset, limit)` | `tg user photos <ID>` |
| Get user status | (from `User.status`) | Shown in `tg user <ID>` |
| Get current user | `getMe()` → `User` | `tg me` |

User status types: `userStatusEmpty`, `userStatusOnline(expires)`, `userStatusOffline(was_online)`, `userStatusRecently(by_my_privacy_settings)`, `userStatusLastWeek(by_my_privacy_settings)`, `userStatusLastMonth(by_my_privacy_settings)`

#### User Actions
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Block user | `setChatBlockList(chat_id, blockListMain)` | `tg block <USER_ID>` |
| Unblock user | `setChatBlockList(chat_id, null)` | `tg unblock <USER_ID>` |
| Get blocked users | `getBlockedMessageSenders(block_list, offset, limit)` | `tg blocked [--limit N]` |

---

## 6. Groups & Supergroups

### Currently Implemented
- List groups (basic groups + supergroups)
- Find group by name
- Send message to group

### Missing - To Implement

#### Group Info
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get basic group info | `getBasicGroup(basic_group_id)` → `BasicGroup` | Shown in `tg chat <ID>` |
| Get basic group full info | `getBasicGroupFullInfo(basic_group_id)` → `BasicGroupFullInfo` | `tg chat <ID> --full` |
| Get supergroup info | `getSupergroup(supergroup_id)` → `Supergroup` | Shown in `tg chat <ID>` |
| Get supergroup full info | `getSupergroupFullInfo(supergroup_id)` → `SupergroupFullInfo` | `tg chat <ID> --full` |

#### Group Creation & Management
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Create basic group | `createNewBasicGroupChat(user_ids, title, message_auto_delete_time)` | `tg group create "Name" --members <ID1,ID2>` |
| Create supergroup | `createNewSupergroupChat(title, is_forum, is_channel, description, location, message_auto_delete_time, for_import)` | `tg group create "Name" --super [--forum]` |
| Upgrade to supergroup | `upgradeBasicGroupChatToSupergroupChat(chat_id)` | `tg group upgrade <CHAT_ID>` |

#### Member Management
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Add member | `addChatMember(chat_id, user_id, forward_limit)` | `tg group add-member <CHAT_ID> <USER_ID>` |
| Add members | `addChatMembers(chat_id, user_ids)` | `tg group add-members <CHAT_ID> <IDS...>` |
| Remove member | `banChatMember(chat_id, member_id, banned_until_date, revoke_messages)` then `unbanChatMember(...)` | `tg group kick <CHAT_ID> <USER_ID>` |
| Ban member | `banChatMember(chat_id, member_id, banned_until_date, revoke_messages)` | `tg group ban <CHAT_ID> <USER_ID> [--until <DATE>]` |
| Unban member | `unbanChatMember(chat_id, member_id)` | `tg group unban <CHAT_ID> <USER_ID>` |
| Restrict member | `setChatMemberStatus(chat_id, member_id, chatMemberStatusRestricted(...))` | `tg group restrict <CHAT_ID> <USER_ID> [--perms ...]` |
| Promote to admin | `setChatMemberStatus(chat_id, member_id, chatMemberStatusAdministrator(...))` | `tg group promote <CHAT_ID> <USER_ID> [--rights ...]` |
| Demote admin | `setChatMemberStatus(chat_id, member_id, chatMemberStatusMember)` | `tg group demote <CHAT_ID> <USER_ID>` |
| Transfer ownership | `transferChatOwnership(chat_id, user_id, password)` | `tg group transfer <CHAT_ID> <USER_ID>` |

Admin rights: `can_manage_chat`, `can_change_info`, `can_post_messages`, `can_edit_messages`, `can_delete_messages`, `can_invite_users`, `can_restrict_members`, `can_pin_messages`, `can_manage_topics`, `can_promote_members`, `can_manage_video_chats`, `can_post_stories`, `can_edit_stories`, `can_delete_stories`, `is_anonymous`, `custom_title`

Restricted permissions: `can_send_basic_messages`, `can_send_audios`, `can_send_documents`, `can_send_photos`, `can_send_videos`, `can_send_video_notes`, `can_send_voice_notes`, `can_send_polls`, `can_send_other_messages` (stickers/GIFs), `can_add_link_previews`, `can_change_info`, `can_invite_users`, `can_pin_messages`, `can_create_topics`

#### Supergroup-Specific
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get supergroup members | `getSupergroupMembers(supergroup_id, filter, offset, limit)` | `tg group members <CHAT_ID> [--filter ...]` |
| Set slow mode | `setChatSlowModeDelay(chat_id, slow_mode_delay)` | `tg group slow-mode <CHAT_ID> <SECONDS>` |
| Toggle all history | `toggleSupergroupIsAllHistoryAvailable(supergroup_id, is_all_history_available)` | `tg group toggle-history <CHAT_ID>` |
| Toggle aggressive anti-spam | `toggleSupergroupHasAggressiveAntiSpamEnabled(supergroup_id, enabled)` | `tg group toggle-anti-spam <CHAT_ID>` |
| Toggle hide members | `toggleSupergroupHasHiddenMembers(supergroup_id, has_hidden_members)` | `tg group toggle-hide-members <CHAT_ID>` |
| Set sticker set | `setChatAvailableReactions(chat_id, available_reactions)` | `tg group set-reactions <CHAT_ID> [--all\|--some ...]` |

Member filters: `supergroupMembersFilterRecent`, `supergroupMembersFilterAdministrators`, `supergroupMembersFilterSearch(query)`, `supergroupMembersFilterRestricted(query)`, `supergroupMembersFilterBanned(query)`, `supergroupMembersFilterBots`, `supergroupMembersFilterContacts(query)`

---

## 7. Channels

### Currently Implemented
- Listed in groups output
- Can send messages to channels

### Missing - To Implement

#### Channel Management
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Create channel | `createNewSupergroupChat(title, false, true, description, ...)` | `tg channel create "Name" [--description "desc"]` |
| Set linked discussion group | `setChatDiscussionGroup(chat_id, discussion_chat_id)` | `tg channel set-discussion <CHAN_ID> <GROUP_ID>` |
| Get discussion group | From `SupergroupFullInfo.linked_chat_id` | Shown in `tg chat <CHAN_ID> --full` |
| Toggle signatures | `toggleSupergroupSignMessages(supergroup_id, sign_messages)` | `tg channel toggle-signatures <CHAN_ID>` |
| Toggle join-to-send | `toggleSupergroupJoinToSendMessages(supergroup_id, join_to_send)` | `tg channel toggle-join-to-send <CHAN_ID>` |
| Toggle join-by-request | `toggleSupergroupJoinByRequest(supergroup_id, join_by_request)` | `tg channel toggle-join-request <CHAN_ID>` |
| Set chat username | `setSupergroupUsername(supergroup_id, username)` | `tg channel set-username <CHAN_ID> <USERNAME>` |
| Toggle active username | `toggleSupergroupUsernameIsActive(supergroup_id, username, is_active)` | `tg channel toggle-username <CHAN_ID> <USERNAME>` |

#### Channel Statistics
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get channel stats | `getChatStatistics(chat_id, is_dark)` → `ChatStatistics` | `tg channel stats <CHAN_ID>` |
| Get message stats | `getMessageStatistics(chat_id, message_id, is_dark)` | `tg channel stats <CHAN_ID> --message <MSG_ID>` |
| Get story stats | `getStoryStatistics(chat_id, story_id, is_dark)` | `tg channel stats <CHAN_ID> --story <STORY_ID>` |

---

## 8. Media & Files

### Currently Implemented
- Download single file from message
- MIME type detection and filename generation
- SHA256 deduplication
- Priority queuing (1-32)

### Missing - To Implement

#### File Upload
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Upload file | `preliminaryUploadFile(file, file_type, priority)` | Internal (used by send photo/video/doc) |
| Cancel upload | `cancelPreliminaryUploadFile(file_id)` | `tg upload cancel <FILE_ID>` |
| Get file | `getFile(file_id)` → `File` | `tg file <FILE_ID>` |
| Get remote file | `getRemoteFile(remote_file_id, file_type)` → `File` | `tg file --remote <REMOTE_ID>` |

#### File Download (Enhanced)
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Download file (async) | `downloadFile(file_id, priority, offset, limit, synchronous: false)` | Default behavior |
| Cancel download | `cancelDownloadFile(file_id, only_if_pending)` | `tg download cancel <FILE_ID>` |
| Delete downloaded file | `deleteFile(file_id)` | `tg file delete <FILE_ID>` |
| Get file download | `getFileDownloadedPrefixSize(file_id, offset)` → `FileDownloadedPrefixSize` | Internal progress tracking |

#### Downloaded Files Management
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Add file to downloads | `addFileToDownloads(file_id, chat_id, message_id, priority)` | `tg downloads add <CHAT_ID> <MSG_ID>` |
| Remove from downloads | `removeFileFromDownloads(file_id, delete_from_cache)` | `tg downloads remove <FILE_ID>` |
| Remove all downloads | `removeAllFilesFromDownloads(only_active, only_completed, delete_from_cache)` | `tg downloads clear [--active\|--completed]` |
| Search downloads | `searchFileDownloads(query, only_active, only_completed, offset, limit)` | `tg downloads [--query Q]` |

---

## 9. Stickers & Custom Emoji

### Currently Implemented
- Display sticker emoji in message output

### Missing - To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get sticker set | `getStickerSet(set_id)` → `StickerSet` | `tg stickers <SET_ID>` |
| Search sticker set | `searchStickerSet(name)` | `tg stickers search <NAME>` |
| Search installed stickers | `searchInstalledStickerSets(sticker_type, query, limit)` | `tg stickers installed [--query Q]` |
| Get trending stickers | `getTrendingStickerSets(sticker_type, offset, limit)` | `tg stickers trending` |
| Search stickers | `searchStickers(sticker_type, emojis, query, input_language_codes, offset, limit)` | `tg stickers find <EMOJI>` |
| Get recent stickers | `getRecentStickers(is_attached)` | `tg stickers recent` |
| Get favorite stickers | `getFavoriteStickers()` | `tg stickers favorites` |
| Add favorite sticker | `addFavoriteSticker(sticker)` | `tg stickers fav-add <FILE_ID>` |
| Remove favorite sticker | `removeFavoriteSticker(sticker)` | `tg stickers fav-remove <FILE_ID>` |
| Install sticker set | `changeStickerSet(set_id, is_installed: true, is_archived: false)` | `tg stickers install <SET_ID>` |
| Uninstall sticker set | `changeStickerSet(set_id, is_installed: false, is_archived: false)` | `tg stickers uninstall <SET_ID>` |
| Get custom emoji | `getCustomEmojiStickers(custom_emoji_ids)` | Internal (for display) |
| Get emoji categories | `getEmojiCategories(type)` | `tg emoji categories` |
| Search emoji | `searchEmojis(text, input_language_codes)` | `tg emoji search <TEXT>` |
| Get saved animations | `getSavedAnimations()` | `tg animations` |

---

## 10. Reactions

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Add reaction | `addMessageReaction(chat_id, message_id, reaction_type, is_big, update_recent_reactions)` | `tg react <CHAT_ID> <MSG_ID> <EMOJI>` |
| Remove reaction | `removeMessageReaction(chat_id, message_id, reaction_type)` | `tg react <CHAT_ID> <MSG_ID> <EMOJI> --remove` |
| Get message reactions | `getMessageAddedReactions(chat_id, message_id, reaction_type, offset, limit)` | `tg reactions <CHAT_ID> <MSG_ID>` |
| Get available reactions | `getMessageAvailableReactions(chat_id, message_id, row_size)` | Internal |
| Set chat reactions | `setChatAvailableReactions(chat_id, available_reactions)` | `tg chat set-reactions <CHAT_ID>` |
| Set default reaction | `setDefaultReaction(reaction_type)` | `tg settings default-reaction <EMOJI>` |
| Read all reactions | `readAllChatReactions(chat_id)` | `tg chat read-reactions <CHAT_ID>` |
| Set paid reaction privacy | `setPaidMessageReactionType(chat_id, type)` | Advanced setting |
| Get emoji reaction | `getEmojiReaction(emoji)` → `EmojiReaction` | Internal |

---

## 11. Polls

### Currently Implemented
- Display poll in messages output

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Send poll | `sendMessage(...)` with `inputMessagePoll(question, options, is_anonymous, type, open_period, close_date, is_closed)` | `tg send <TARGET> --poll "Question?" --options "A" "B" "C" [--quiz --correct 0]` |
| Vote on poll | `setPollAnswer(chat_id, message_id, option_ids)` | `tg vote <CHAT_ID> <MSG_ID> <OPTION_IDS...>` |
| Retract vote | `setPollAnswer(chat_id, message_id, [])` | `tg vote <CHAT_ID> <MSG_ID> --retract` |
| Stop poll | `stopPoll(chat_id, message_id, reply_markup)` | `tg poll stop <CHAT_ID> <MSG_ID>` |
| Get poll voters | `getPollVoters(chat_id, message_id, option_id, offset, limit)` | `tg poll voters <CHAT_ID> <MSG_ID> <OPTION_ID>` |

---

## 12. Search

### Currently Implemented
- Contact search by name (`search_contacts`)

### Missing - To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Search messages globally | `searchMessages(chat_list, only_in_channels, query, offset, limit, filter, min_date, max_date)` | `tg search messages "query" [--filter photos\|videos\|docs] [--from DATE] [--to DATE]` |
| Search chat messages | `searchChatMessages(chat_id, query, sender_id, from_message_id, offset, limit, filter, message_thread_id, saved_messages_topic_id)` | `tg search <CHAT_ID> "query" [--filter ...]` |
| Search secret chat messages | `searchSecretMessages(chat_id, query, offset, limit, filter)` | `tg search --secret <CHAT_ID> "query"` |
| Search public chats | `searchPublicChats(query)` | `tg search --public "query"` |
| Search chats | `searchChats(query, limit)` | `tg search --chats "query"` |
| Search chats on server | `searchChatsOnServer(query, limit)` | `tg search --chats-server "query"` |
| Search recently found chats | `searchRecentlyFoundChats(query, limit)` | `tg search --recent "query"` |
| Get recently found chats | `getRecentlyFoundChats(limit)` | `tg search --recent` |
| Get recently opened chats | `getRecentlyOpenedChats(limit)` | `tg search --opened` |
| Search hashtag messages | `searchHashtags(prefix, limit)` | `tg search --hashtag "prefix"` |
| Count messages | `getChatMessageCount(chat_id, filter, saved_messages_topic_id, return_local)` | `tg search <CHAT_ID> --count [--filter ...]` |
| Get message position | `getChatMessagePosition(chat_id, message_id, filter, message_thread_id, saved_messages_topic_id)` | Internal |

Search filters: `searchMessagesFilterEmpty`, `searchMessagesFilterAnimation`, `searchMessagesFilterAudio`, `searchMessagesFilterDocument`, `searchMessagesFilterPhoto`, `searchMessagesFilterVideo`, `searchMessagesFilterVoiceNote`, `searchMessagesFilterPhotoAndVideo`, `searchMessagesFilterUrl`, `searchMessagesFilterChatPhoto`, `searchMessagesFilterVideoNote`, `searchMessagesFilterVoiceAndVideoNote`, `searchMessagesFilterMention`, `searchMessagesFilterUnreadMention`, `searchMessagesFilterUnreadReaction`, `searchMessagesFilterFailedToSend`, `searchMessagesFilterPinned`

---

## 13. Notifications

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get notification settings | `getScopeNotificationSettings(scope)` | `tg notifications` |
| Set notification settings | `setScopeNotificationSettings(scope, settings)` | `tg notifications set --scope <private\|group\|channel> [options]` |
| Set chat notifications | `setChatNotificationSettings(chat_id, settings)` | `tg chat mute/unmute <CHAT_ID>` |
| Get notification groups | (monitor `updateNotificationGroup`) | Internal |
| Remove notification | `removeNotification(notification_group_id, notification_id)` | Internal |
| Remove notification group | `removeNotificationGroup(notification_group_id, max_notification_id)` | Internal |

Notification scopes: `notificationSettingsScopePrivateChats`, `notificationSettingsScopeGroupChats`, `notificationSettingsScopeChannelChats`

---

## 14. Chat Folders

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get chat folders | `getChatFolders()` → `ChatFolders` | `tg folders` |
| Create folder | `createChatFolder(folder)` | `tg folder create "Name" [options]` |
| Edit folder | `editChatFolder(chat_folder_id, folder)` | `tg folder edit <ID> [options]` |
| Delete folder | `deleteChatFolder(chat_folder_id, leave_chat_ids)` | `tg folder delete <ID>` |
| Reorder folders | `reorderChatFolders(chat_folder_ids, main_chat_list_position)` | `tg folder reorder <IDS...>` |
| Get chats in folder | `getChats(chatListFolder(folder_id), limit)` | `tg chats --folder <ID>` |
| Add chat to folder | `addChatToList(chat_id, chatListFolder(folder_id))` | `tg folder add <FOLDER_ID> <CHAT_ID>` |
| Get recommended folders | `getRecommendedChatFolders()` | `tg folders --recommended` |
| Share folder | `createChatFolderInviteLink(chat_folder_id, name, chat_ids)` | `tg folder share <ID>` |
| Get folder invite links | `getChatFolderInviteLinks(chat_folder_id)` | `tg folder links <ID>` |

ChatFolder structure: `title`, `icon`, `color_id`, `is_shareable`, `pinned_chat_ids`, `included_chat_ids`, `excluded_chat_ids`, `exclude_muted`, `exclude_read`, `exclude_archived`, `include_contacts`, `include_non_contacts`, `include_bots`, `include_groups`, `include_channels`

---

## 15. Forum Topics

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get forum topics | `getForumTopics(chat_id, query, offset_date, offset_message_id, offset_message_thread_id, limit)` | `tg topics <CHAT_ID>` |
| Create topic | `createForumTopic(chat_id, name, icon)` | `tg topic create <CHAT_ID> "Name" [--icon <EMOJI>]` |
| Edit topic | `editForumTopic(chat_id, message_thread_id, name, edit_icon_custom_emoji, icon_custom_emoji_id)` | `tg topic edit <CHAT_ID> <TOPIC_ID> "New Name"` |
| Get topic | `getForumTopic(chat_id, message_thread_id)` | `tg topic <CHAT_ID> <TOPIC_ID>` |
| Get topic link | `getForumTopicLink(chat_id, message_thread_id)` | `tg topic link <CHAT_ID> <TOPIC_ID>` |
| Close topic | `toggleForumTopicIsClosed(chat_id, message_thread_id, is_closed: true)` | `tg topic close <CHAT_ID> <TOPIC_ID>` |
| Reopen topic | `toggleForumTopicIsClosed(chat_id, message_thread_id, is_closed: false)` | `tg topic reopen <CHAT_ID> <TOPIC_ID>` |
| Hide general topic | `toggleGeneralForumTopicIsHidden(chat_id, is_hidden: true)` | `tg topic hide-general <CHAT_ID>` |
| Pin topic | `toggleForumTopicIsPinned(chat_id, message_thread_id, is_pinned)` | `tg topic pin <CHAT_ID> <TOPIC_ID>` |
| Delete topic | `deleteForumTopic(chat_id, message_thread_id)` | `tg topic delete <CHAT_ID> <TOPIC_ID>` |
| Get topic icons | `getForumTopicDefaultIcons()` | `tg topic icons` |
| Read all topics | `readAllChatMentions(chat_id)` | Internal |
| Toggle forum mode | `toggleSupergroupIsForum(supergroup_id, is_forum)` | `tg group toggle-forum <CHAT_ID>` |

---

## 16. Stories

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get chat stories | `getChatActiveStories(chat_id)` → `ChatActiveStories` | `tg stories <CHAT_ID>` |
| Get story | `getStory(story_sender_chat_id, story_id, only_local)` | `tg story <CHAT_ID> <STORY_ID>` |
| Send story | `sendStory(chat_id, content, areas, caption, privacy_settings, active_period, from_story_full_id, is_posted_to_chat_page, protect_content)` | `tg story send <CHAT_ID> <FILE> [options]` |
| Edit story | `editStory(story_sender_chat_id, story_id, content, areas, caption)` | `tg story edit <CHAT_ID> <STORY_ID> [options]` |
| Delete story | `deleteStory(story_sender_chat_id, story_id)` | `tg story delete <CHAT_ID> <STORY_ID>` |
| Get story viewers | `getStoryViewers(story_id, query, only_contacts, prefer_forwards, prefer_with_reaction, offset, limit)` | `tg story viewers <STORY_ID>` |
| Open story | `openStory(story_sender_chat_id, story_id)` | Internal (before viewing) |
| Close story | `closeStory(story_sender_chat_id, story_id)` | Internal (after viewing) |
| Get stories archive | `getChatArchivedStories(chat_id, from_story_id, limit)` | `tg stories <CHAT_ID> --archived` |
| Get pinned stories | `getChatPinnedStories(chat_id, from_story_id, limit)` | `tg stories <CHAT_ID> --pinned` |
| Get story stealth mode | `getStoryStealthMode()` | `tg story stealth-mode` |
| Activate stealth mode | `activateStoryStealthMode()` | `tg story stealth` |
| Toggle pinned | `toggleStoryIsPostedToChatPage(story_sender_chat_id, story_id, is_posted_to_chat_page)` | `tg story pin <CHAT_ID> <STORY_ID>` |
| Set story privacy | `setStoryPrivacySettings(story_id, privacy_settings)` | `tg story privacy <STORY_ID> [options]` |
| Repost story | `sendStory(...)` with `storyFullId` source | `tg story repost <CHAT_ID> <STORY_ID> <TARGET_CHAT>` |

Story content: `inputStoryContentPhoto(photo, added_sticker_file_ids)`, `inputStoryContentVideo(video, added_sticker_file_ids, duration, cover_frame_timestamp, is_animation)`

Story privacy: `storyPrivacySettingsEveryone`, `storyPrivacySettingsContacts`, `storyPrivacySettingsCloseFriends`, `storyPrivacySettingsSelectedUsers(user_ids)`

---

## 17. Calls

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Create call | `createCall(user_id, protocol, is_video)` | `tg call <USER_ID> [--video]` |
| Accept call | `acceptCall(call_id, protocol)` | `tg call accept <CALL_ID>` |
| Discard call | `discardCall(call_id, is_disconnected, duration, is_video, connection_id)` | `tg call end <CALL_ID>` |
| Send call rating | `sendCallRating(call_id, rating, comment, problems)` | `tg call rate <CALL_ID> <1-5>` |
| Send call debug | `sendCallDebugInformation(call_id, debug_information)` | Internal |
| Send call log | `sendCallLog(call_id, log_file)` | Internal |

#### Group Calls
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Create group call | `createVideoChat(chat_id, title, start_date, is_rtmp_stream)` | `tg call group <CHAT_ID> [--title "Title"]` |
| Get group call | `getGroupCall(group_call_id)` | `tg call info <CALL_ID>` |
| Join group call | `joinGroupCall(group_call_id, participant_id, audio_source_id, payload, is_muted, is_my_video_enabled, invite_hash)` | `tg call join <CALL_ID>` |
| Leave group call | `leaveGroupCall(group_call_id)` | `tg call leave <CALL_ID>` |
| End group call | `endGroupCall(group_call_id)` | `tg call end-group <CALL_ID>` |
| Toggle mute | `toggleGroupCallParticipantIsMuted(group_call_id, participant_id, is_muted)` | `tg call mute <CALL_ID> [USER_ID]` |
| Set title | `setGroupCallTitle(group_call_id, title)` | `tg call title <CALL_ID> "Title"` |
| Get RTMP URL | `getVideoChatRtmpUrl(chat_id)` | `tg call rtmp <CHAT_ID>` |
| Invite users | `inviteGroupCallParticipants(group_call_id, user_ids)` | `tg call invite <CALL_ID> <USER_IDS...>` |
| Get participants | `loadGroupCallParticipants(group_call_id, limit)` | `tg call participants <CALL_ID>` |
| Schedule call | `createVideoChat(chat_id, title, start_date, ...)` | `tg call group <CHAT_ID> --schedule "2024-01-01T12:00:00"` |
| Start scheduled | `startScheduledGroupCall(group_call_id)` | `tg call start <CALL_ID>` |
| Toggle recording | `toggleGroupCallScreenSharingIsPaused(group_call_id, is_paused)` | `tg call record <CALL_ID>` |

---

## 18. Secret Chats

### Currently Implemented
- Type detection only

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Create secret chat | `createNewSecretChat(user_id)` | `tg secret create <USER_ID>` |
| Get secret chat | `getSecretChat(secret_chat_id)` → `SecretChat` | `tg secret <ID>` |
| Close secret chat | `closeSecretChat(secret_chat_id)` | `tg secret close <ID>` |
| Send to secret chat | Standard `sendMessage` with secret chat's `chat_id` | `tg send --secret <ID> -m "text"` |
| Self-destruct messages | `inputMessageContent` with `self_destruct_type` | `tg send --secret <ID> -m "text" --ttl <SECONDS>` |

Secret chat states: `secretChatStatePending`, `secretChatStateReady`, `secretChatStateClosed`

---

## 19. Scheduled Messages

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Send scheduled | `sendMessage(...)` with `messageSendOptions(scheduling_state: messageSchedulingStateSendAtDate(send_date))` | `tg send <TARGET> -m "text" --schedule "2024-01-01T12:00:00"` |
| Send when online | `sendMessage(...)` with `messageSendOptions(scheduling_state: messageSchedulingStateSendWhenOnline)` | `tg send <TARGET> -m "text" --when-online` |
| Get scheduled | `getChatScheduledMessages(chat_id)` | `tg scheduled <CHAT_ID>` |
| Edit scheduled | `editMessageSchedulingState(chat_id, message_id, scheduling_state)` | `tg scheduled edit <CHAT_ID> <MSG_ID> --date "..."` |
| Delete scheduled | `deleteMessages(chat_id, message_ids, true)` | `tg scheduled delete <CHAT_ID> <MSG_IDS...>` |

---

## 20. Saved Messages

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Save message (forward to self) | `forwardMessages(self_chat_id, ...)` | `tg save <CHAT_ID> <MSG_ID>` |
| Search saved | `searchSavedMessages(topic_id, tag, query, from_message_id, offset, limit)` | `tg saved search "query"` |
| Get saved tags | `getSavedMessagesTags(topic_id)` | `tg saved tags` |
| Set tag label | `setSavedMessagesTagLabel(tag, label)` | `tg saved tag-label <TAG> "label"` |
| Get topic history | `getSavedMessagesTopicHistory(topic_id, from_message_id, offset, limit)` | `tg saved <TOPIC_ID>` |
| Load topics | `loadSavedMessagesTopics(limit)` | `tg saved topics` |
| Pin topic | `toggleSavedMessagesTopicIsPinned(topic_id, is_pinned)` | `tg saved pin <TOPIC_ID>` |
| Delete topic history | `deleteSavedMessagesTopicHistory(topic_id)` | `tg saved delete <TOPIC_ID>` |

---

## 21. Account & Settings

### Currently Implemented
- None (beyond auth)

### To Implement

#### Profile Management
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get current user | `getMe()` → `User` | `tg me` |
| Set name | `setName(first_name, last_name)` | `tg me set-name "First" "Last"` |
| Set bio | `setBio(bio)` | `tg me set-bio "bio text"` |
| Set username | `setUsername(username)` | `tg me set-username "username"` |
| Toggle username active | `toggleUsernameIsActive(username, is_active)` | `tg me toggle-username <USERNAME>` |
| Reorder usernames | `reorderActiveUsernames(usernames)` | `tg me reorder-usernames <U1> <U2> ...` |
| Set profile photo | `setProfilePhoto(photo, is_public)` | `tg me set-photo <FILE>` |
| Delete profile photo | `deleteProfilePhoto(profile_photo_id)` | `tg me delete-photo <PHOTO_ID>` |
| Set personal chat | `setChatPersonalChat(user_id, personal_chat_id)` | `tg me set-personal-chat <CHAT_ID>` |
| Set birthdate | `setBirthdate(birthdate)` | `tg me set-birthday <DATE>` |
| Set emoji status | `setEmojiStatus(emoji_status)` | `tg me set-emoji-status <EMOJI_ID>` |
| Set accent color | `setAccentColor(accent_color_id, background_custom_emoji_id)` | `tg me set-color <COLOR_ID>` |

#### Account Settings
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get account TTL | `getAccountTtl()` | `tg me ttl` |
| Set account TTL | `setAccountTtl(ttl)` | `tg me ttl <DAYS>` |
| Delete account | `deleteAccount(reason, password)` | `tg me delete-account --reason "reason"` |
| Get auto-download settings | `getAutoDownloadSettingsPresets()` | `tg settings auto-download` |
| Set auto-download settings | `setAutoDownloadSettings(settings, type)` | `tg settings auto-download set [options]` |

#### Content Settings
| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get user link | `getUserLink()` → `UserLink` | `tg me link` |
| Get support user | `getSupportUser()` → `User` | `tg support` |
| Get suggested actions | `getSuggestedFileName(file_id, directory)` | Internal |

---

## 22. Privacy & Security

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get privacy setting | `getUserPrivacySettingRules(setting)` → `UserPrivacySettingRules` | `tg privacy <SETTING>` |
| Set privacy setting | `setUserPrivacySettingRules(setting, rules)` | `tg privacy <SETTING> set [options]` |
| Get close friends | `getCloseFriends()` → `Users` | `tg privacy close-friends` |
| Set close friends | `setCloseFriends(user_ids)` | `tg privacy close-friends set <IDS...>` |

Privacy settings: `userPrivacySettingShowStatus`, `userPrivacySettingShowProfilePhoto`, `userPrivacySettingShowLinkInForwardedMessages`, `userPrivacySettingShowPhoneNumber`, `userPrivacySettingShowBio`, `userPrivacySettingShowBirthdate`, `userPrivacySettingAllowChatInvites`, `userPrivacySettingAllowCalls`, `userPrivacySettingAllowPeerToPeerCalls`, `userPrivacySettingAllowFindingByPhoneNumber`, `userPrivacySettingAllowPrivateVoiceAndVideoNoteMessages`, `userPrivacySettingAllowUnpaidMessages`

Privacy rule types: `userPrivacySettingRuleAllowAll`, `userPrivacySettingRuleAllowContacts`, `userPrivacySettingRuleAllowPremiumUsers`, `userPrivacySettingRuleAllowUsers(user_ids)`, `userPrivacySettingRuleAllowChatMembers(chat_ids)`, `userPrivacySettingRuleRestrictAll`, `userPrivacySettingRuleRestrictContacts`, `userPrivacySettingRuleRestrictUsers(user_ids)`, `userPrivacySettingRuleRestrictChatMembers(chat_ids)`

---

## 23. Contacts Management

### Currently Implemented
- Search contacts

### Missing - To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get contacts | `getContacts()` → `Users` | `tg contacts` |
| Add contact | `addContact(contact, share_phone_number)` | `tg contacts add <PHONE> <FIRST> [LAST]` |
| Import contacts | `importContacts(contacts)` | `tg contacts import <FILE.vcf>` |
| Remove contacts | `removeContacts(user_ids)` | `tg contacts remove <IDS...>` |
| Get imported count | `getImportedContactCount()` | `tg contacts count` |
| Change imported contacts | `changeImportedContacts(contacts)` | Bulk contact update |
| Clear imported contacts | `clearImportedContacts()` | `tg contacts clear-imported` |
| Share phone number | `sharePhoneNumber(user_id)` | `tg contacts share-phone <USER_ID>` |
| Get recently found | `getRecentlyFoundChats(limit)` | `tg contacts recent` |
| Clear recently found | `clearRecentlyFoundChats()` | `tg contacts clear-recent` |

---

## 24. Inline Mode

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get inline query results | `getInlineQueryResults(bot_user_id, chat_id, user_location, query, offset)` → `InlineQueryResults` | `tg inline <BOT_USERNAME> "query"` |
| Send inline result | `sendInlineQueryResultMessage(chat_id, message_thread_id, reply_to, options, query_id, result_id, hide_via_bot)` | `tg send <TARGET> --inline <BOT> "query" --result <ID>` |

---

## 25. Payments & Stars

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get payment form | `getPaymentForm(input_invoice, theme)` | `tg payment form <CHAT_ID> <MSG_ID>` |
| Validate order | `validateOrderInfo(input_invoice, order_info, allow_save)` | Internal |
| Send payment | `sendPaymentForm(input_invoice, payment_form_id, order_info_id, shipping_option_id, credentials, tip_amount)` | `tg payment send <CHAT_ID> <MSG_ID>` (interactive) |
| Get receipt | `getPaymentReceipt(chat_id, message_id)` | `tg payment receipt <CHAT_ID> <MSG_ID>` |
| Get Star balance | `getStarBalance()` | `tg stars balance` |
| Get Star transactions | `getStarTransactions(subscription_id, direction, offset, limit)` | `tg stars transactions` |
| Get Star revenue | `getStarRevenueStatistics(owner_id, is_dark)` | `tg stars revenue <CHAT_ID>` |
| Send stars | `sendGift(...)` | Through gift system |

---

## 26. Premium

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get premium state | `getPremiumState()` → `PremiumState` | `tg premium` |
| Get premium features | `getPremiumFeatures(source)` | `tg premium features` |
| Get premium limits | `getPremiumLimit(limit_type)` | `tg premium limits` |
| Apply gift code | `applyPremiumGiftCode(code)` | `tg premium redeem <CODE>` |
| Check gift code | `checkPremiumGiftCode(code)` → `PremiumGiftCodeInfo` | `tg premium check <CODE>` |
| Can purchase | `canPurchaseFromStore(purpose)` | Internal |
| Toggle sponsored | `toggleHasSponsoredMessagesEnabled(enabled)` | `tg premium toggle-ads` |

---

## 27. Backgrounds & Themes

### Currently Implemented
- None (terminal-based, limited applicability)

### To Implement (Low Priority)

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get backgrounds | `getInstalledBackgrounds(for_dark_theme)` | `tg backgrounds` |
| Search background | `searchBackground(name)` | `tg backgrounds search <NAME>` |
| Set background | `setDefaultBackground(background, type, for_dark_theme)` | `tg backgrounds set <NAME>` |
| Remove background | `removeInstalledBackground(background_id)` | `tg backgrounds remove <ID>` |

---

## 28. Language Packs

### Currently Implemented
- None (CLI outputs English)

### To Implement (Low Priority)

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get language packs | `getLocalizationTargetInfo(only_local)` | `tg languages` |
| Get language info | `getLanguagePackInfo(language_pack_id)` | `tg languages <ID>` |
| Get strings | `getLanguagePackStrings(language_pack_id, keys)` | Internal |

---

## 29. Proxy & Network

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Add proxy | `addProxy(server, port, enable, type)` | `tg proxy add <SERVER> <PORT> --type socks5\|http\|mtproto [--enable]` |
| Edit proxy | `editProxy(proxy_id, server, port, enable, type)` | `tg proxy edit <ID> [options]` |
| Enable proxy | `enableProxy(proxy_id)` | `tg proxy enable <ID>` |
| Disable proxy | `disableProxy()` | `tg proxy disable` |
| Remove proxy | `removeProxy(proxy_id)` | `tg proxy remove <ID>` |
| List proxies | `getProxies()` | `tg proxy list` |
| Ping proxy | `pingProxy(proxy_id)` | `tg proxy ping <ID>` |
| Test proxy | `testProxy(server, port, type, dc_id, timeout)` | `tg proxy test <SERVER> <PORT>` |
| Get network stats | `getNetworkStatistics(only_current)` | `tg network stats` |
| Set network type | `setNetworkType(type)` | `tg network type <wifi\|mobile\|other\|none>` |

Proxy types: `proxyTypeSocks5(username, password)`, `proxyTypeHttp(username, password, http_only)`, `proxyTypeMtproto(secret)`

---

## 30. Statistics & Analytics

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get chat statistics | `getChatStatistics(chat_id, is_dark)` → `ChatStatistics` | `tg stats <CHAT_ID>` |
| Get message statistics | `getMessageStatistics(chat_id, message_id, is_dark)` | `tg stats message <CHAT_ID> <MSG_ID>` |
| Get story statistics | `getStoryStatistics(chat_id, story_id, is_dark)` | `tg stats story <CHAT_ID> <STORY_ID>` |
| Get statistics graph | `getStatisticalGraph(chat_id, token, x)` | Internal (data for above) |
| Get chat boost status | `getChatBoostStatus(chat_id)` → `ChatBoostStatus` | `tg boost status <CHAT_ID>` |
| Get boosts | `getChatBoosts(chat_id, only_gift_codes, offset, limit)` | `tg boost list <CHAT_ID>` |
| Boost chat | `boostChat(chat_id, slot_ids)` | `tg boost <CHAT_ID>` |
| Get boost slots | `getAvailableChatBoostSlots()` | `tg boost slots` |
| Get chat revenue | `getChatRevenueStatistics(chat_id, is_dark)` | `tg revenue <CHAT_ID>` |
| Get revenue transactions | `getChatRevenueTransactions(chat_id, offset, limit)` | `tg revenue transactions <CHAT_ID>` |
| Get revenue withdrawal URL | `getChatRevenueWithdrawalUrl(chat_id, password)` | `tg revenue withdraw <CHAT_ID>` |

---

## 31. Deep Links

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Parse deep link | `getInternalLinkType(link)` → `InternalLinkType` | `tg link parse <URL>` |
| Get deep link info | `getDeepLinkInfo(link)` | `tg link info <URL>` |
| Build internal link | `getInternalLink(type)` → `HttpUrl` | Internal |
| Get external link | `getExternalLink(link, allow_write_access)` | Internal |
| Get web page preview | `getWebPagePreview(text, link_preview_options)` | `tg link preview <URL>` |
| Get instant view | `getWebPageInstantView(url, force_full)` | `tg link view <URL>` |

---

## 32. Web Apps (Mini Apps)

### Currently Implemented
- None

### To Implement (Limited CLI Applicability)

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Open web app | `openWebApp(chat_id, bot_user_id, url, ...)` | `tg webapp open <BOT_ID> [URL]` |
| Get web app URL | `getWebAppUrl(bot_user_id, url, ...)` | `tg webapp url <BOT_ID>` |
| Search web app | `searchWebApp(bot_user_id, web_app_short_name)` | `tg webapp search <BOT_ID> <NAME>` |

---

## 33. Sponsored Messages & Ads

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get sponsored messages | `getChatSponsoredMessages(chat_id)` | `tg sponsored <CHAT_ID>` |
| Click sponsored | `clickChatSponsoredMessage(chat_id, message_id, ...)` | Internal |
| Report sponsored | `reportChatSponsoredMessage(chat_id, message_id, option_id)` | `tg sponsored report <CHAT_ID> <MSG_ID>` |

---

## 34. Gifts

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get available gifts | `getAvailableGifts()` | `tg gifts` |
| Send gift | `sendGift(gift_id, user_id, text, is_private, pay_for_upgrade)` | `tg gift send <USER_ID> <GIFT_ID>` |
| Get received gifts | `getReceivedGifts(user_id, offset, limit)` | `tg gifts received [USER_ID]` |
| Sell gift | `sellGift(...)` | `tg gift sell <...>` |
| Toggle gift saved | `toggleGiftIsSaved(sender_user_id, message_id, is_saved)` | `tg gift save <MSG_ID>` |
| Transfer gift | `transferGift(...)` | `tg gift transfer <...>` |
| Upgrade gift | `upgradeGift(...)` | `tg gift upgrade <...>` |
| Set gift settings | `setGiftSettings(...)` | `tg gift settings` |

---

## 35. Passport

### Currently Implemented
- None

### To Implement (Low Priority)

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Get passport element | `getPassportElement(type, password)` | `tg passport <TYPE>` |
| Get all elements | `getAllPassportElements(password)` | `tg passport list` |
| Set passport element | `setPassportElement(element, password)` | `tg passport set <TYPE> [options]` |
| Delete passport element | `deletePassportElement(type)` | `tg passport delete <TYPE>` |
| Set passport errors | `setPassportElementErrors(user_id, errors)` | Internal (for bots) |
| Send auth form | `sendPassportAuthorizationForm(authorization_form_id, types)` | `tg passport authorize <FORM_ID>` |
| Get auth form | `getPassportAuthorizationForm(bot_user_id, scope, public_key, nonce)` | Internal |

---

## 36. Quick Reply Shortcuts

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Add shortcut message | `addQuickReplyShortcutMessage(shortcut_name, reply_to_message_id, input_message_content)` | `tg shortcuts add <NAME> -m "text"` |
| Edit shortcut message | `editQuickReplyMessage(shortcut_id, message_id, input_message_content)` | `tg shortcuts edit <ID> <MSG_ID> -m "new text"` |
| Delete shortcut | `deleteQuickReplyShortcut(shortcut_id)` | `tg shortcuts delete <ID>` |
| Load shortcuts | `loadQuickReplyShortcuts()` | `tg shortcuts` |
| Send shortcut | `sendQuickReplyShortcutMessages(chat_id, shortcut_id, sending_id)` | `tg shortcuts send <CHAT_ID> <SHORTCUT_ID>` |
| Rename shortcut | `setQuickReplyShortcutName(shortcut_id, name)` | `tg shortcuts rename <ID> "new name"` |
| Reorder shortcuts | `reorderQuickReplyShortcuts(shortcut_ids)` | `tg shortcuts reorder <IDS...>` |

---

## 37. Reporting

### Currently Implemented
- None

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Report chat | `reportChat(chat_id, option_id, message_ids, text)` | `tg report chat <CHAT_ID> --reason <REASON>` |
| Report chat photo | `reportChatPhoto(chat_id, file_id, reason, text)` | `tg report photo <CHAT_ID> <FILE_ID>` |
| Report story | `reportStory(story_sender_chat_id, story_id, option_id, text)` | `tg report story <CHAT_ID> <STORY_ID>` |

---

## 38. Logging & Debug

### Currently Implemented
- None (TDLib log level set during init)

### To Implement

| Feature | TDLib Functions | CLI UX |
|---------|----------------|--------|
| Set log level | `setLogVerbosityLevel(new_verbosity_level)` | `tg debug --log-level <0-5>` |
| Get log level | `getLogVerbosityLevel()` | `tg debug --log-level` |
| Set log tag level | `setLogTagVerbosityLevel(tag, level)` | `tg debug --tag <TAG> --level <N>` |
| Get log tags | `getLogTags()` | `tg debug --tags` |
| Set log stream | `setLogStream(log_stream)` | `tg debug --log-file <PATH>` |
| Get TDLib version | `getOption("version")` | `tg version --tdlib` |
| Get current state | `getCurrentState()` → `Updates` | `tg debug --state` |

---

## 39. Implementation Priority

### Tier 1 — Core Messaging (High Impact, Moderate Effort)

These complete the basic messaging experience:

1. **Send media messages** — photo, video, document, audio, voice, animation, video note
2. **Forward messages** — `forwardMessages`
3. **Reply to messages** — `sendMessage` with `inputMessageReplyToMessage`
4. **Edit messages** — `editMessageText`, `editMessageCaption`
5. **Delete messages** — `deleteMessages`
6. **Pin/unpin messages** — `pinChatMessage`, `unpinChatMessage`
7. **Send with formatting** — markdown/HTML entity parsing
8. **Handle all message types** — venue, game, invoice, dice, service messages
9. **Message threads** — `getMessageThread`, `getMessageThreadHistory`

### Tier 2 — Chat Management (High Impact, Moderate Effort)

10. **Chat details** — `getChat`, full info, member count, admins
11. **Archive/unarchive** — `addChatToList`
12. **Pin/unpin chats** — `toggleChatIsPinned`
13. **Mute/unmute** — `setChatNotificationSettings`
14. **Delete/clear chat** — `deleteChatHistory`
15. **Leave chat** — `leaveChat`
16. **Block/unblock** — `setChatBlockList`
17. **Chat folders** — full CRUD

### Tier 3 — Search & Discovery (High Impact, Low-Moderate Effort)

18. **Global message search** — `searchMessages`
19. **In-chat message search** — `searchChatMessages`
20. **Public chat search** — `searchPublicChats`
21. **Chat search** — `searchChats`, `searchChatsOnServer`

### Tier 4 — User & Contact Management (Medium Impact)

22. **User profile** — `getMe`, `getUser`, `getUserFullInfo`
23. **Profile management** — set name, bio, username, photo
24. **Contacts CRUD** — add, remove, import, list
25. **Privacy settings** — get/set all 13 privacy settings

### Tier 5 — Group Administration (Medium Impact, Higher Effort)

26. **Create groups/channels** — `createNewBasicGroupChat`, `createNewSupergroupChat`
27. **Member management** — add, remove, ban, restrict, promote
28. **Chat invite links** — create, edit, revoke, list
29. **Join requests** — approve, deny
30. **Supergroup settings** — slow mode, history visibility, anti-spam

### Tier 6 — Reactions & Interactions (Medium Impact, Low Effort)

31. **Reactions** — add, remove, list
32. **Polls** — create, vote, stop, get voters
33. **Scheduled messages** — send, list, edit, delete

### Tier 7 — Media Features (Medium Impact, Moderate Effort)

34. **File upload** — `preliminaryUploadFile`
35. **Download management** — add/remove/list downloads
36. **Sticker operations** — search, install, favorites
37. **Stories** — view, send, edit, delete

### Tier 8 — Account & Security (Lower Impact but Important)

38. **Session management** — list, terminate, confirm
39. **Password management** — set, remove, recover
40. **QR code auth** — alternative login method
41. **Proxy management** — add, edit, enable, disable

### Tier 9 — Advanced Features (Lower Impact)

42. **Calls** — create, accept, end (voice/video/group)
43. **Forum topics** — CRUD operations
44. **Secret chats** — create, manage
45. **Saved messages** — search, tags, topics
46. **Inline mode** — query bots inline
47. **Quick replies** — shortcuts management

### Tier 10 — Specialized Features (Lowest Priority)

48. **Statistics** — chat, message, story stats
49. **Payments & Stars** — view forms, receipts, balance
50. **Premium** — status, features, gift codes
51. **Gifts** — send, receive, manage
52. **Backgrounds & themes** — limited CLI use
53. **Language packs** — limited CLI use
54. **Passport** — identity verification
55. **Web Apps** — limited CLI use
56. **Deep links** — parse, preview
57. **Reporting** — report chats, photos, stories
58. **Sponsored messages** — view, report
59. **Affiliate programs** — limited CLI use
60. **TON integration** — blockchain features

---

## Appendix A: TDLib Update Types to Monitor

Beyond the currently monitored updates (`AuthorizationState`, `MessageSendSucceeded`, `MessageSendFailed`), the following updates should be handled for a complete client:

### Critical Updates
| Update | Purpose |
|--------|---------|
| `updateNewMessage` | New incoming message |
| `updateMessageContent` | Message content changed (edited) |
| `updateDeleteMessages` | Messages deleted |
| `updateChatLastMessage` | Chat's last message changed |
| `updateChatReadInbox` | Inbox read pointer moved |
| `updateChatReadOutbox` | Outbox read pointer moved |
| `updateChatUnreadMentionCount` | Unread mentions changed |
| `updateChatUnreadReactionCount` | Unread reactions changed |
| `updateChatPosition` | Chat position in list changed |
| `updateChatTitle` | Chat title changed |
| `updateChatPhoto` | Chat photo changed |

### Important Updates
| Update | Purpose |
|--------|---------|
| `updateUser` | User info changed |
| `updateUserStatus` | Online/offline status changed |
| `updateFile` | File download progress |
| `updateNewChat` | New chat appeared |
| `updateChatNotificationSettings` | Notification settings changed |
| `updateChatDraftMessage` | Draft message changed |
| `updateChatIsMarkedAsUnread` | Unread mark changed |
| `updateChatBlockList` | Block status changed |
| `updateChatFolders` | Chat folders changed |
| `updateNotification` | New notification |
| `updateNotificationGroup` | Notification group changed |
| `updateMessageReaction` | Reaction on message changed |
| `updateMessageReactions` | All reactions changed |
| `updateChatAction` | Typing/recording indicators |
| `updateStory` | Story changed |
| `updateChatActiveStories` | Chat active stories changed |

### Call Updates
| Update | Purpose |
|--------|---------|
| `updateCall` | Call state changed |
| `updateNewCallSignalingData` | WebRTC signaling |
| `updateGroupCall` | Group call changed |
| `updateGroupCallParticipant` | Participant joined/left/changed |

### Other Updates
| Update | Purpose |
|--------|---------|
| `updateSecretChat` | Secret chat state changed |
| `updateConnectionState` | Network connection state |
| `updateOption` | TDLib option changed |
| `updateSelectedBackground` | Background changed |
| `updateLanguagePackStrings` | Localization updated |
| `updateTermsOfService` | ToS needs acceptance |
| `updateSuggestedActions` | Suggested actions available |
| `updateUnconfirmedSession` | New session needs confirmation |

---

## Appendix B: TelegramClient Trait Extension

The `TelegramClient` trait needs significant expansion. Suggested method groupings:

```rust
#[async_trait]
pub trait TelegramClient {
    // === Auth ===
    async fn authenticate(&mut self, phone: Option<&str>) -> Result<()>;
    async fn authenticate_qr(&mut self) -> Result<String>; // returns QR link
    async fn is_authenticated(&self) -> bool;
    async fn logout(&mut self) -> Result<()>;

    // === Me ===
    async fn get_me(&self) -> Result<UserInfo>;
    async fn set_name(&self, first: &str, last: &str) -> Result<()>;
    async fn set_bio(&self, bio: &str) -> Result<()>;
    async fn set_username(&self, username: &str) -> Result<()>;
    async fn set_profile_photo(&self, path: &str) -> Result<()>;

    // === Chats ===
    async fn get_chats(&self, list: ChatListType, limit: i32) -> Result<Vec<ChatInfo>>;
    async fn get_chat_details(&self, chat_id: i64) -> Result<ChatDetails>;
    async fn archive_chat(&self, chat_id: i64, archive: bool) -> Result<()>;
    async fn pin_chat(&self, chat_id: i64, pin: bool) -> Result<()>;
    async fn mute_chat(&self, chat_id: i64, mute_for: i32) -> Result<()>;
    async fn delete_chat_history(&self, chat_id: i64, revoke: bool) -> Result<()>;
    async fn leave_chat(&self, chat_id: i64) -> Result<()>;
    async fn block_chat(&self, chat_id: i64, block: bool) -> Result<()>;
    async fn set_chat_title(&self, chat_id: i64, title: &str) -> Result<()>;
    async fn set_chat_description(&self, chat_id: i64, desc: &str) -> Result<()>;
    async fn set_chat_ttl(&self, chat_id: i64, seconds: i32) -> Result<()>;

    // === Messages ===
    async fn get_messages(&self, chat_id: i64, limit: i32) -> Result<Vec<MessageInfo>>;
    async fn get_message(&self, chat_id: i64, message_id: i64) -> Result<MessageInfo>;
    async fn send_message(&self, chat_id: i64, content: MessageContent) -> Result<SendResult>;
    async fn send_album(&self, chat_id: i64, contents: Vec<MessageContent>) -> Result<Vec<SendResult>>;
    async fn edit_message_text(&self, chat_id: i64, msg_id: i64, text: &str) -> Result<()>;
    async fn edit_message_caption(&self, chat_id: i64, msg_id: i64, caption: &str) -> Result<()>;
    async fn delete_messages(&self, chat_id: i64, msg_ids: &[i64], revoke: bool) -> Result<()>;
    async fn forward_messages(&self, from_chat: i64, msg_ids: &[i64], to_chat: i64) -> Result<Vec<SendResult>>;
    async fn pin_message(&self, chat_id: i64, msg_id: i64, silent: bool) -> Result<()>;
    async fn unpin_message(&self, chat_id: i64, msg_id: i64) -> Result<()>;
    async fn unpin_all_messages(&self, chat_id: i64) -> Result<()>;
    async fn get_message_thread(&self, chat_id: i64, msg_id: i64, limit: i32) -> Result<Vec<MessageInfo>>;
    async fn get_message_link(&self, chat_id: i64, msg_id: i64) -> Result<String>;
    async fn translate_message(&self, chat_id: i64, msg_id: i64, lang: &str) -> Result<String>;
    async fn mark_chat_as_read(&self, chat_id: i64) -> Result<()>;
    async fn mark_chat_as_unread(&self, chat_id: i64) -> Result<()>;

    // === Reactions ===
    async fn add_reaction(&self, chat_id: i64, msg_id: i64, emoji: &str) -> Result<()>;
    async fn remove_reaction(&self, chat_id: i64, msg_id: i64, emoji: &str) -> Result<()>;
    async fn get_reactions(&self, chat_id: i64, msg_id: i64) -> Result<Vec<ReactionInfo>>;

    // === Search ===
    async fn search_contacts(&self, query: &str) -> Result<Vec<ContactInfo>>;
    async fn search_messages_global(&self, query: &str, filter: Option<SearchFilter>, limit: i32) -> Result<Vec<MessageInfo>>;
    async fn search_chat_messages(&self, chat_id: i64, query: &str, filter: Option<SearchFilter>, limit: i32) -> Result<Vec<MessageInfo>>;
    async fn search_public_chats(&self, query: &str) -> Result<Vec<ChatInfo>>;

    // === Files ===
    async fn download_message_media(&self, chat_id: i64, msg_id: i64, opts: DownloadOptions) -> Result<DownloadReport>;
    async fn upload_file(&self, path: &str) -> Result<FileInfo>;
    async fn cancel_download(&self, file_id: i32) -> Result<()>;

    // === Groups ===
    async fn create_group(&self, title: &str, user_ids: &[i64]) -> Result<i64>;
    async fn create_supergroup(&self, title: &str, is_channel: bool, description: &str) -> Result<i64>;
    async fn add_chat_member(&self, chat_id: i64, user_id: i64) -> Result<()>;
    async fn ban_chat_member(&self, chat_id: i64, user_id: i64, until: i32) -> Result<()>;
    async fn unban_chat_member(&self, chat_id: i64, user_id: i64) -> Result<()>;
    async fn promote_chat_member(&self, chat_id: i64, user_id: i64, rights: AdminRights) -> Result<()>;
    async fn restrict_chat_member(&self, chat_id: i64, user_id: i64, perms: ChatPermissions) -> Result<()>;
    async fn get_chat_members(&self, chat_id: i64, filter: MemberFilter, limit: i32) -> Result<Vec<MemberInfo>>;
    async fn get_chat_admins(&self, chat_id: i64) -> Result<Vec<MemberInfo>>;

    // === Invite Links ===
    async fn create_invite_link(&self, chat_id: i64, opts: InviteLinkOptions) -> Result<InviteLink>;
    async fn revoke_invite_link(&self, chat_id: i64, link: &str) -> Result<()>;
    async fn get_invite_links(&self, chat_id: i64) -> Result<Vec<InviteLink>>;
    async fn process_join_request(&self, chat_id: i64, user_id: i64, approve: bool) -> Result<()>;

    // === Chat Folders ===
    async fn get_chat_folders(&self) -> Result<Vec<ChatFolder>>;
    async fn create_chat_folder(&self, folder: ChatFolderSpec) -> Result<i32>;
    async fn edit_chat_folder(&self, folder_id: i32, folder: ChatFolderSpec) -> Result<()>;
    async fn delete_chat_folder(&self, folder_id: i32) -> Result<()>;

    // === Contacts ===
    async fn get_contacts(&self) -> Result<Vec<ContactInfo>>;
    async fn add_contact(&self, phone: &str, first: &str, last: &str) -> Result<()>;
    async fn remove_contacts(&self, user_ids: &[i64]) -> Result<()>;
    async fn import_contacts(&self, contacts: Vec<ContactSpec>) -> Result<ImportResult>;

    // === Scheduled Messages ===
    async fn get_scheduled_messages(&self, chat_id: i64) -> Result<Vec<MessageInfo>>;
    async fn send_scheduled(&self, chat_id: i64, content: MessageContent, send_date: i32) -> Result<SendResult>;
    async fn edit_schedule(&self, chat_id: i64, msg_id: i64, send_date: i32) -> Result<()>;

    // === Stories ===
    async fn get_active_stories(&self, chat_id: i64) -> Result<Vec<StoryInfo>>;
    async fn send_story(&self, chat_id: i64, content: StoryContent, opts: StoryOptions) -> Result<StoryInfo>;
    async fn delete_story(&self, chat_id: i64, story_id: i32) -> Result<()>;
    async fn get_story_viewers(&self, story_id: i32, limit: i32) -> Result<Vec<ViewerInfo>>;

    // === Forum Topics ===
    async fn get_forum_topics(&self, chat_id: i64, limit: i32) -> Result<Vec<TopicInfo>>;
    async fn create_forum_topic(&self, chat_id: i64, name: &str, icon: Option<&str>) -> Result<TopicInfo>;
    async fn edit_forum_topic(&self, chat_id: i64, topic_id: i64, name: &str) -> Result<()>;
    async fn close_forum_topic(&self, chat_id: i64, topic_id: i64, close: bool) -> Result<()>;
    async fn delete_forum_topic(&self, chat_id: i64, topic_id: i64) -> Result<()>;

    // === Polls ===
    async fn send_poll(&self, chat_id: i64, question: &str, options: Vec<String>, opts: PollOptions) -> Result<SendResult>;
    async fn vote_poll(&self, chat_id: i64, msg_id: i64, option_ids: &[i32]) -> Result<()>;
    async fn stop_poll(&self, chat_id: i64, msg_id: i64) -> Result<()>;

    // === Sessions ===
    async fn get_active_sessions(&self) -> Result<Vec<SessionInfo>>;
    async fn terminate_session(&self, session_id: i64) -> Result<()>;
    async fn terminate_all_other_sessions(&self) -> Result<()>;

    // === Privacy ===
    async fn get_privacy_setting(&self, setting: PrivacySetting) -> Result<PrivacyRules>;
    async fn set_privacy_setting(&self, setting: PrivacySetting, rules: PrivacyRules) -> Result<()>;
    async fn get_blocked_users(&self, limit: i32) -> Result<Vec<UserInfo>>;

    // === Secret Chats ===
    async fn create_secret_chat(&self, user_id: i64) -> Result<i64>;
    async fn close_secret_chat(&self, secret_chat_id: i32) -> Result<()>;

    // === Calls ===
    async fn create_call(&self, user_id: i64, is_video: bool) -> Result<i32>;
    async fn accept_call(&self, call_id: i32) -> Result<()>;
    async fn discard_call(&self, call_id: i32) -> Result<()>;
    async fn create_video_chat(&self, chat_id: i64, title: &str) -> Result<i32>;

    // === Proxy ===
    async fn add_proxy(&self, server: &str, port: i32, proxy_type: ProxyType, enable: bool) -> Result<i32>;
    async fn get_proxies(&self) -> Result<Vec<ProxyInfo>>;
    async fn enable_proxy(&self, proxy_id: i32) -> Result<()>;
    async fn disable_proxy(&self) -> Result<()>;
    async fn remove_proxy(&self, proxy_id: i32) -> Result<()>;
    async fn ping_proxy(&self, proxy_id: i32) -> Result<f64>;

    // === Stickers ===
    async fn get_sticker_set(&self, set_id: i64) -> Result<StickerSetInfo>;
    async fn search_stickers(&self, emoji: &str) -> Result<Vec<StickerInfo>>;
    async fn get_installed_sticker_sets(&self) -> Result<Vec<StickerSetInfo>>;
    async fn install_sticker_set(&self, set_id: i64) -> Result<()>;
    async fn uninstall_sticker_set(&self, set_id: i64) -> Result<()>;

    // === Notifications ===
    async fn get_notification_settings(&self, scope: NotificationScope) -> Result<NotificationSettings>;
    async fn set_notification_settings(&self, scope: NotificationScope, settings: NotificationSettings) -> Result<()>;

    // === Statistics ===
    async fn get_chat_statistics(&self, chat_id: i64) -> Result<ChatStatistics>;
    async fn get_chat_boost_status(&self, chat_id: i64) -> Result<BoostStatus>;

    // === Saved Messages ===
    async fn search_saved_messages(&self, query: &str, limit: i32) -> Result<Vec<MessageInfo>>;
    async fn get_saved_tags(&self) -> Result<Vec<SavedTag>>;

    // === Premium ===
    async fn get_premium_state(&self) -> Result<PremiumState>;

    // === Inline ===
    async fn get_inline_results(&self, bot_id: i64, chat_id: i64, query: &str) -> Result<InlineResults>;
    async fn send_inline_result(&self, chat_id: i64, query_id: i64, result_id: &str) -> Result<SendResult>;

    // === Misc ===
    async fn get_option(&self, name: &str) -> Result<String>;
    async fn find_chat_by_name(&self, name: &str) -> Result<i64>;
    async fn find_group_by_name(&self, name: &str) -> Result<i64>;
}
```

---

## Appendix C: New CLI Subcommand Summary

Total new subcommands to add (organized by group):

```
tg me                          # Profile info and management
tg user <ID>                   # View user details
tg contacts                    # Contact management
tg blocked                     # Blocked users list

tg chat <ID>                   # Chat details and management
tg join <INVITE_LINK>          # Join via invite link
tg invite                      # Invite link management

tg edit <CHAT> <MSG> "text"    # Edit message
tg delete <CHAT> <MSGS...>    # Delete messages
tg forward <FROM> <MSG> <TO>  # Forward message
tg pin/unpin                   # Pin message management
tg thread <CHAT> <MSG>         # Message threads
tg message <CHAT> <MSG>        # Single message details

tg react <CHAT> <MSG> <EMOJI>  # Add/remove reactions
tg reactions <CHAT> <MSG>      # View reactions
tg vote <CHAT> <MSG> <OPT>     # Vote on polls

tg folders                     # Chat folder management
tg topics <CHAT>               # Forum topics
tg stories <CHAT>              # Stories
tg story                       # Story operations

tg scheduled <CHAT>            # Scheduled messages
tg saved                       # Saved messages

tg sessions                    # Active sessions
tg websites                    # Connected websites
tg password                    # Password management
tg privacy                     # Privacy settings

tg group                       # Group administration subcommands
tg channel                     # Channel management subcommands

tg call                        # Voice/video/group calls
tg secret                      # Secret chats

tg proxy                       # Proxy management
tg network                     # Network stats/settings

tg stickers                    # Sticker operations
tg emoji                       # Emoji search/categories
tg animations                  # Saved animations

tg translate                   # Text/message translation
tg transcribe                  # Voice-to-text

tg stats <CHAT>                # Chat statistics
tg boost                       # Boost operations
tg revenue <CHAT>              # Revenue statistics

tg premium                     # Premium status/features
tg gifts                       # Gift operations
tg stars                       # Star balance/transactions

tg downloads                   # Download management
tg shortcuts                   # Quick reply shortcuts

tg link                        # Deep link operations
tg webapp                      # Web app operations
tg inline <BOT> "query"        # Inline bot queries

tg report                      # Report content
tg sponsored                   # Sponsored messages

tg debug                       # Logging and debug
tg version                     # Version info (already partially exists)
```

---

## Appendix D: Message Content Union Type

For the expanded `send` command, define a `MessageContent` enum:

```rust
pub enum MessageContent {
    Text {
        text: String,
        parse_mode: Option<ParseMode>,  // Markdown, HTML, None
    },
    Photo {
        path: String,
        caption: Option<String>,
        has_spoiler: bool,
        self_destruct: Option<i32>,
    },
    Video {
        path: String,
        caption: Option<String>,
        has_spoiler: bool,
        supports_streaming: bool,
    },
    Document {
        path: String,
        caption: Option<String>,
    },
    Audio {
        path: String,
        caption: Option<String>,
        title: Option<String>,
        performer: Option<String>,
    },
    VoiceNote {
        path: String,
        caption: Option<String>,
    },
    VideoNote {
        path: String,
    },
    Animation {
        path: String,
        caption: Option<String>,
    },
    Sticker {
        path_or_id: String,
    },
    Location {
        latitude: f64,
        longitude: f64,
        live_period: Option<i32>,
        heading: Option<i32>,
        proximity_alert_radius: Option<i32>,
    },
    Contact {
        phone: String,
        first_name: String,
        last_name: Option<String>,
        vcard: Option<String>,
    },
    Poll {
        question: String,
        options: Vec<String>,
        is_anonymous: bool,
        poll_type: PollType,
        open_period: Option<i32>,
    },
    Dice {
        emoji: String,  // one of: 🎲🎯🏀⚽🎳🎰
    },
    Venue {
        latitude: f64,
        longitude: f64,
        title: String,
        address: String,
        provider: Option<String>,
        id: Option<String>,
    },
}

pub enum ParseMode {
    Markdown,
    Html,
}

pub enum PollType {
    Regular { allow_multiple: bool },
    Quiz { correct_option: i32, explanation: Option<String> },
}
```

---

## Appendix E: Send Options

```rust
pub struct SendOptions {
    pub reply_to: Option<i64>,           // message_id to reply to
    pub message_thread_id: Option<i64>,  // forum topic thread
    pub schedule: Option<ScheduleOption>,
    pub disable_notification: bool,      // send silently
    pub protect_content: bool,           // disable forwarding
    pub effect_id: Option<i64>,          // message effect
    pub send_when_online: bool,          // schedule for when recipient online
}

pub enum ScheduleOption {
    AtDate(i32),      // unix timestamp
    WhenOnline,       // send when user comes online
}
```
