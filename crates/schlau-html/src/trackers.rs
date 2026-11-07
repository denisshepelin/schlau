use std::collections::HashMap;
use std::sync::LazyLock;

use regex::RegexSet;

/// How a host in the built-in list behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackerKind {
    /// Exists to track opens or clicks; any image loaded from it is a pixel.
    Tracking,
    /// Email service provider or analytics vendor that also hosts real content.
    /// Its images load under `RemotePolicy::Allow` but not under `AllowExceptTrackers`.
    Sender,
}

const TRACKING_HOSTS: &[&str] = &[
    // Mailchimp
    "list-manage.com",
    "list-manage1.com",
    "list-manage2.com",
    "eep.io",
    // Twilio SendGrid click tracking subdomains
    "ct.sendgrid.net",
    // HubSpot
    "hubspotemail.net",
    "hubspotlinks.com",
    "hubspotstarter.net",
    "hs-analytics.net",
    "track.hubspot.com",
    "t.sidekickopen01.com",
    "t.sidekickopen02.com",
    "t.sidekickopen03.com",
    "t.sidekickopen04.com",
    "t.sidekickopen05.com",
    "t.sidekickopen06.com",
    "t.sidekickopen07.com",
    "t.signaux.com",
    "t.senal.com",
    "t.signauxtrois.com",
    // Salesforce Marketing Cloud / Pardot
    "exct.net",
    "pi.pardot.com",
    "go.pardot.com",
    // Klaviyo
    "trk.klaviyo.com",
    "trk.klaviyomail.com",
    "klclick.com",
    "klclick1.com",
    "klclick2.com",
    "klclick3.com",
    "ctrk.klclick.com",
    // customer.io
    "customeriomail.com",
    "track.customer.io",
    // Intercom
    "via.intercom.io",
    "intercom-mail.com",
    // Sales engagement and personal open trackers
    "mailtrack.io",
    "bananatag.com",
    "t.yesware.com",
    "yesware.com",
    "track.mixmax.com",
    "email-tracking.mixmax.com",
    "r.superhuman.com",
    "mailfoogae.appspot.com",
    "streak.com",
    "getnotify.com",
    "saleshandy.com",
    "contactmonkey.com",
    "cirrusinsight.com",
    "mailbutler.io",
    "trk.mailbutler.io",
    "gmass.co",
    "readnotify.com",
    "didtheyreadit.com",
    "pointofmail.com",
    "whoreadme.com",
    "spypig.com",
    "polymail.io",
    "close.io",
    "outreach.io",
    "salesloft.com",
    "lemlist.com",
    "mailshake.com",
    "reply.io",
    "woodpecker.co",
    "boomeranggmail.com",
    // Litmus and inbox analytics
    "emltrk.com",
    "pixel.app.returnpath.net",
    "returnpath.net",
    // Analytics and ad pixels
    "google-analytics.com",
    "googletagmanager.com",
    "doubleclick.net",
    "googleadservices.com",
    "googlesyndication.com",
    "connect.facebook.net",
    "px.ads.linkedin.com",
    "ads.linkedin.com",
    "analytics.twitter.com",
    "bat.bing.com",
    "ct.pinterest.com",
    "api.mixpanel.com",
    "api.segment.io",
    "liveintent.com",
    // Constant Contact
    "rs6.net",
    // Campaign Monitor
    "cmail1.com",
    "cmail2.com",
    "cmail19.com",
    "cmail20.com",
    "createsend1.com",
    // SparkPost
    "sparkpostmail.com",
    "spgo.io",
    // Postmark
    "pstmrk.it",
    // Amazon SES
    "awstrack.me",
    // Mailjet
    "mjt.lu",
    // Brevo (Sendinblue)
    "sendibt2.com",
    "sendibt3.com",
    "sendibt4.com",
    "sendibw.com",
    "sendibm1.com",
    "sendibm2.com",
    // Oracle Eloqua / Responsys
    "en25.com",
    "rsys2.net",
    "rsys4.net",
    "rsys5.net",
    // ConvertKit
    "convertkit-mail.com",
    "convertkit-mail2.com",
    "convertkit-mail3.com",
    "convertkit-mail4.com",
    // MailerLite
    "mlsend.com",
    "mlsend2.com",
    // dotdigital
    "trackedlink.net",
    "dmtrk.net",
    // Cheetah Digital
    "chtah.com",
    "chtah.net",
    // Emarsys
    "emarsys.net",
    // Selligent
    "slgnt.eu",
    "slgnt.us",
    // Adobe Campaign
    "neolane.net",
    // Zoho Campaigns
    "zcsend.net",
    "maillist-manage.com",
    // Iterable and Braze link hosts
    "links.iterable.com",
    "appboy.com",
];

const SENDER_HOSTS: &[&str] = &[
    "mailchimp.com",
    "mcusercontent.com",
    "sendgrid.net",
    "sendgrid.com",
    "mandrillapp.com",
    "mailgun.net",
    "mailgun.org",
    "hubspot.com",
    "hubspotusercontent-na1.net",
    "hubspotusercontent.net",
    "exacttarget.com",
    "sfmc-content.com",
    "pardot.com",
    "klaviyo.com",
    "customer.io",
    "intercom.io",
    "intercomcdn.com",
    "mixmax.com",
    "constantcontact.com",
    "ctctcdn.com",
    "createsend.com",
    "sparkpost.com",
    "postmarkapp.com",
    "braze.com",
    "braze-images.com",
    "iterable.com",
    "mailjet.com",
    "sendinblue.com",
    "brevo.com",
    "marketo.com",
    "mktoweb.com",
    "mktdns.com",
    "eloqua.com",
    "responsys.net",
    "convertkit.com",
    "mailerlite.com",
    "aweber.com",
    "getresponse.com",
    "activehosted.com",
    "activecampaign.com",
    "drip.com",
    "omnisend.com",
    "sailthru.com",
    "bronto.com",
    "emarsys.com",
    "movableink.com",
    "getvero.com",
    "beehiiv.com",
    "substack.com",
    "zohocampaigns.com",
];

/// Host plus path prefix rules for vendors whose main domain serves real pages.
const HOST_PATH_RULES: &[(&str, &str)] = &[
    ("facebook.com", "/tr"),
    ("google.com", "/pagead/"),
    ("google.com", "/ads/"),
    ("linkedin.com", "/px"),
    ("t.co", "/i/adsct"),
    ("twitter.com", "/i/adsct"),
    ("x.com", "/i/adsct"),
    ("reddit.com", "/rp.gif"),
];

/// Path patterns that open-tracking pixels use across vendors and self-hosted tools.
const PIXEL_PATH_PATTERNS: &[&str] = &[
    r"(?:^|/)open(?:\.(?:gif|png|php|aspx?|jsp))?(?:$|/)",
    r"(?:^|/)opens?/",
    r"/track/open",
    r"/tracking/open",
    r"/trk/open",
    r"/e/o/",
    r"^/o/",
    r"^/wf/open",
    r"^/on\.jsp",
    r"^/tn\.jsp",
    r"(?:^|/)o\.gif$",
    r"(?:^|/)t\.gif$",
    r"(?:^|/)p\.gif$",
    r"(?:^|/)spacer-?track",
    r"(?:^|/)(?:tracking-?)?pixel(?:\.(?:gif|png|jpe?g|php|aspx?))?(?:$|/)",
    r"(?:^|/)beacon(?:\.(?:gif|png|php))?(?:$|/)",
    r"(?:^|/)imp(?:ression)?(?:\.(?:gif|png|php))?$",
    r"(?:^|/)email[_-]?open",
    r"(?:^|/)mail[_-]?open",
    r"(?:^|/)lt\.php$",
    r"(?:^|/)read[_-]?receipt",
];

static HOST_TABLE: LazyLock<HashMap<&'static str, TrackerKind>> = LazyLock::new(|| {
    SENDER_HOSTS
        .iter()
        .map(|host| (*host, TrackerKind::Sender))
        .chain(
            TRACKING_HOSTS
                .iter()
                .map(|host| (*host, TrackerKind::Tracking)),
        )
        .collect()
});

static PIXEL_PATHS: LazyLock<RegexSet> =
    LazyLock::new(|| RegexSet::new(PIXEL_PATH_PATTERNS).expect("pixel path patterns compile"));

/// Every domain and pattern in the built-in list, for documentation and tests.
pub fn tracker_host_count() -> usize {
    TRACKING_HOSTS.len() + SENDER_HOSTS.len() + HOST_PATH_RULES.len()
}

fn host_suffixes(host: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(host), |rest| {
        rest.split_once('.').map(|(_, tail)| tail)
    })
    .filter(|suffix| suffix.contains('.'))
}

/// Looks up a host (lowercase, no port) by its most specific listed suffix.
pub fn host_kind(host: &str) -> Option<TrackerKind> {
    host_suffixes(host).find_map(|suffix| HOST_TABLE.get(suffix).copied())
}

fn matches_host_rule(host: &str, rule_host: &str) -> bool {
    host == rule_host
        || host
            .strip_suffix(rule_host)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// Classifies a remote resource by host and path. `path` is the URL path, lowercase.
pub fn classify(host: &str, path: &str) -> Option<TrackerKind> {
    let pixel_path = PIXEL_PATHS.is_match(path)
        || HOST_PATH_RULES.iter().any(|(rule_host, prefix)| {
            matches_host_rule(host, rule_host) && path.starts_with(prefix)
        });
    if pixel_path {
        return Some(TrackerKind::Tracking);
    }
    host_kind(host)
}

/// The registrable-looking part of a host reported to the user: the listed suffix when
/// known, else the last two labels.
pub fn report_domain(host: &str) -> String {
    host_suffixes(host)
        .find(|suffix| HOST_TABLE.contains_key(suffix))
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let labels: Vec<&str> = host.rsplitn(3, '.').collect();
            match labels.as_slice() {
                [tld, name, ..] => format!("{name}.{tld}"),
                _ => host.to_owned(),
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_lookup_prefers_specific_entries() {
        assert_eq!(host_kind("ct.sendgrid.net"), Some(TrackerKind::Tracking));
        assert_eq!(
            host_kind("u123.ct.sendgrid.net"),
            Some(TrackerKind::Tracking)
        );
        assert_eq!(
            host_kind("cdn.mcauto-images-production.sendgrid.net"),
            Some(TrackerKind::Sender)
        );
        assert_eq!(host_kind("example.com"), None);
        assert_eq!(host_kind("notlist-manage.com"), None);
    }

    #[test]
    fn pixel_paths() {
        assert_eq!(
            classify("example.com", "/track/open.php"),
            Some(TrackerKind::Tracking)
        );
        assert_eq!(
            classify("example.com", "/wf/open"),
            Some(TrackerKind::Tracking)
        );
        assert_eq!(
            classify("example.com", "/o.gif"),
            Some(TrackerKind::Tracking)
        );
        assert_eq!(
            classify("www.facebook.com", "/tr"),
            Some(TrackerKind::Tracking)
        );
        assert_eq!(classify("example.com", "/images/openhouse.jpg"), None);
        assert_eq!(classify("example.com", "/img/pixel-art-hero.png"), None);
    }

    #[test]
    fn report_domain_uses_list_entry() {
        assert_eq!(report_domain("us1.list-manage.com"), "list-manage.com");
        assert_eq!(report_domain("img.shop.example.co"), "example.co");
    }
}
