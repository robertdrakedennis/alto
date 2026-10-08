//! The client's localised texts: every fixed message the client shows or
//! sends itself (menu entries, console messages, friend and ignore list
//! replies, chat prefixes, loading stages), in each language the client has
//! texts for.
//!
//! The data is one table, `texts_table`, a row per [`Msg`] in the enum's
//! order and a column per language of [`COLUMNS`]. A message is looked up
//! with [`Msg::for_lang`] (a language without a column has no text) or with
//! [`Msg::get`], which uses the launch language.
//!
//! German has six continuation lines (a long message is split over two
//! chat lines); the other languages have no second line, so those cells
//! are absent.
use crate::ui_text_compare::Language;

/// The languages the table has a column for, in column order.
pub const COLUMNS: [Language; 5] = [
    Language::En,
    Language::De,
    Language::Fr,
    Language::Pt,
    Language::EsMx,
];

/// One localised message. The variants are in table row order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
#[allow(missing_docs, reason = "the table row is the documentation")]
pub enum Msg {
    DebugConsoleInfo,
    DebugConsoleError,
    DeveloperConsoleShortcutInfo,
    DebugConsoleUnknownCommand,
    Cancel,
    NoNamePlayername,
    MembersDesc,
    SwapNoteAtBank,
    LentItemReturn,
    BoughtItemDiscard,
    ShardCombinePrefix,
    ShardCombineSuffix,
    ShardItemCombine,
    Take,
    Drop,
    Ok,
    Select,
    Continue,
    InvalidPlayerName,
    YouCantReportYourself,
    YouAlreadySentASnapshot1,
    YouCannotReportStaffForImpersonation1,
    YouCannotReportStaffForImpersonation2,
    YouCannotReportStaffForImpersonation3,
    AbuseReportReceived,
    UnableToSendSnapshotBusy,
    InvalidName,
    UseMembersServerItem,
    UseMembersServerLocation,
    NothingInterestingHappens,
    ICantReachThat,
    InvalidTeleport,
    UseMembersServerCoord,
    UnableToAddFriendSystem,
    UnableToAddFriendExists,
    UnableToAddIgnoreSystem,
    UnableToAddIgnoreExists,
    FriendlistFullMembers,
    FriendlistFull,
    UnableToDeleteFriend,
    UnableToDeleteIgnore,
    UnableToSendMessageBusy,
    UnableToSendMessageUnavailable1,
    UnableToSendMessageUnavailable2,
    UnableToSendMessageNotFriend1,
    UnableToSendMessageNotFriend2,
    UnableToSendMessagePasswordA,
    UnableToSendMessagePasswordB,
    UnableToSendMessageNoDisplayname1,
    UnableToSendMessageNoDisplayname2,
    SnapshotBufferEmpty1,
    SnapshotBufferEmpty2,
    NameDialogNotFound,
    UnableToSendMessageQuickChat1,
    UnableToSendMessageQuickChat2,
    UnableToSendMessageQuickChatWorld1,
    ChatDisabled,
    Under13FriendsChatPrefix,
    UnableToSendMessageNotInFriendsChat,
    UnableToSendMessageFriendsChatTooLowRank,
    UnableToSendMessageFriendsChatError,
    FriendsChatStillInChannel,
    FriendsChatNotInChannel,
    FriendsChatAttemptingJoin,
    FriendsChatSendingLeaveReq,
    FriendsChatJoinInProgress,
    FriendsChatLeaveInProgress,
    FriendsChatInvalidName,
    FriendsChatNotAvailable,
    FriendsChatJoinSuccessA,
    FriendsChatJoinSuccessAUnder13,
    FriendsChatJoinError,
    FriendsChatJoinAttackBlocked,
    FriendsChatJoinNotExist,
    FriendsChatJoinRoomFull,
    FriendsChatJoinLowRank,
    FriendsChatJoinBanned,
    FriendsChatJoinIgnoreList,
    FriendsChatUserJoined,
    FriendsChatUserLeft,
    FriendsChatUserKicked,
    FriendsChatLeaveKicked,
    FriendsChatLeaveRemoved,
    FriendsChatLeaveDefault,
    FriendsChatEnabledA,
    FriendsChatEnabledB,
    FriendsChatDisabled,
    FriendsChatKickLowRank,
    FriendsChatKickUserHigher,
    FriendsChatKickNotFound,
    FriendsChatKickSuccess,
    FriendsChatKickSuccessReset,
    MutedTemporary,
    MutedTemporaryTimeA,
    MutedTemporaryTimeB,
    MutedTemporaryOneDay,
    MutedPrevent,
    MutedPermanent,
    Loading,
    Profiling,
    ConnectionLost,
    AttemptToReestablish,
    CheckingForUpdates,
    DownloadingUpdates,
    LoadConfig,
    LoadedConfig,
    LoadSprites,
    LoadedSprites,
    LoadWordpack,
    LoadedWordpack,
    LoadInterfaces,
    LoadedInterfaces,
    LoadInterfaceScripts,
    LoadedInterfaceScripts,
    LoadAdditionalFonts,
    LoadedAdditionalFonts,
    LoadWorldMap,
    LoadedWorldMap,
    LoadWorldList,
    LoadedWorldList,
    LoadedClientVariables,
    LoadingEllipsis,
    SnapshotPleaseclose1,
    SnapshotPleaseclose2,
    Systemupdate,
    FriendLogin,
    FriendLogout,
    UnableToFind,
    Use,
    Examine,
    Attack,
    ChooseOption,
    MoreOptions,
    WalkHere,
    FaceHere,
    Level,
    Skill,
    Rating,
    PleaseWait,
    Close,
    MenuSeparator,
    Million,
    MillionShort,
    Thousand,
    ThousandShort,
    From,
    SelfLabel,
    FriendListDupe,
    IgnoreListFullMembers,
    IgnoreListFull,
    IgnoreListDupe,
    FriendCantAddSelf,
    IgnoreCantAddSelf,
    FriendlistTimedSave,
    RemoveIgnore1,
    RemoveIgnore2,
    RemoveFriend1,
    RemoveFriend2,
    Chatcol0,
    Chatcol1,
    Chatcol2,
    Chatcol3,
    Chatcol4,
    Chatcol5,
    Chatcol6,
    Chatcol7,
    Chatcol8,
    Chatcol9,
    Chatcol10,
    Chatcol11,
    Chateffect1,
    Chateffect2,
    Chateffect3,
    Chateffect4,
    Chateffect5,
    UnknownFriendDisplaynamePlaceholder,
}

/// How many messages there are.
pub const COUNT: usize = 176;

impl Msg {
    /// Every message, in row order.
    pub const ALL: [Msg; COUNT] = {
        // The enum is dense from 0, so each row index is its variant.
        let mut all = [Msg::DebugConsoleInfo; COUNT];
        let mut index = 0;
        while index < COUNT {
            // SAFETY: `Msg` is `repr(u8)` with discriminants 0..COUNT.
            all[index] = unsafe { std::mem::transmute::<u8, Msg>(index as u8) };
            index += 1;
        }
        all
    };

    /// The message's text in `language`: none when the language has no
    /// column (Dutch and Spanish have no texts) or the cell is absent.
    #[must_use]
    pub fn for_lang(self, language: Language) -> Option<&'static str> {
        let column = COLUMNS.iter().position(|&l| l == language)?;
        crate::texts_table::TABLE[self as usize][column]
    }

    /// The text as concatenated into a message: an absent text is `"null"`.
    #[must_use]
    pub fn display(self, language: Language) -> &'static str {
        self.for_lang(language).unwrap_or("null")
    }

    /// The text in the launch language (applet parameter 26).
    #[must_use]
    pub fn get(self) -> &'static str {
        self.display(client_language())
    }
}

/// The client language from the launcher parameters.
#[must_use]
pub fn client_language() -> Language {
    crate::applet_params::get()
        .language()
        .ok()
        .flatten()
        .and_then(Language::from_id)
        .unwrap_or(Language::En)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every message has a text in every language the client has, except the
    /// six German-only continuation lines.
    #[test]
    fn every_message_is_present_in_every_language() {
        assert_eq!(crate::texts_table::TABLE.len(), COUNT);
        for (row, msg) in Msg::ALL.iter().enumerate() {
            assert_eq!(*msg as usize, row, "{msg:?} is out of order");
            for language in COLUMNS {
                let text = msg.for_lang(language);
                let continuation = language != Language::De
                    && matches!(
                        msg,
                        Msg::UnableToSendMessageUnavailable2
                            | Msg::UnableToSendMessageNotFriend2
                            | Msg::UnableToSendMessageNoDisplayname2
                            | Msg::SnapshotBufferEmpty2
                            | Msg::UnableToSendMessageQuickChat2
                            | Msg::SnapshotPleaseclose2
                    );
                assert_eq!(
                    text.is_none(),
                    continuation,
                    "{msg:?} in {language:?}: {text:?}"
                );
                assert!(text.is_none_or(|t| !t.is_empty()), "{msg:?} {language:?}");
            }
        }
        assert_eq!(Msg::WalkHere.for_lang(Language::De), Some("Hierhin gehen"));
        assert_eq!(Msg::WalkHere.for_lang(Language::Nl), None);
    }
}
