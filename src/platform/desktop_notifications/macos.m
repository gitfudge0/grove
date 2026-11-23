#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>
#import <dispatch/dispatch.h>
#include <stdint.h>

typedef void (*GroveNotificationActivation)(const void *, uint64_t);

// Delegate state is confined to the main queue, including permission,
// delivery, and activation completions supplied by Notification Center.
@interface GroveNotificationDelegate : NSObject <UNUserNotificationCenterDelegate>
@property(nonatomic, strong) UNUserNotificationCenter *center;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, NSString *> *identifiers;
@property(nonatomic) const void *context;
@property(nonatomic) GroveNotificationActivation activate;
- (void)show:(uint64_t)notificationId title:(NSString *)title body:(NSString *)body;
- (void)close:(uint64_t)notificationId;
- (void)dispose;
@end

@implementation GroveNotificationDelegate
- (void)show:(uint64_t)notificationId title:(NSString *)title body:(NSString *)body {
    [self close:notificationId];
    NSNumber *key = @(notificationId);
    // Native requests can survive an app restart. A fresh identifier keeps
    // those requests and late callbacks from matching any current delivery.
    NSString *identifier = NSUUID.UUID.UUIDString;
    self.identifiers[key] = identifier;

    UNMutableNotificationContent *content = [UNMutableNotificationContent new];
    content.title = title;
    content.body = body;
    content.userInfo = @{@"groveNotification": key};
    UNNotificationRequest *request =
        [UNNotificationRequest requestWithIdentifier:identifier content:content trigger:nil];
    // macOS prompts only on the first request and remembers the response.
    // Asking again lets subsequent notifications honor current OS settings.
    [self.center requestAuthorizationWithOptions:UNAuthorizationOptionAlert
                              completionHandler:^(BOOL granted, NSError *error) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (![self.identifiers[key] isEqualToString:identifier]) return;
            if (!granted || error) {
                [self.identifiers removeObjectForKey:key];
                return;
            }
            [self.center addNotificationRequest:request withCompletionHandler:^(NSError *deliveryError) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (![self.identifiers[key] isEqualToString:identifier]) {
                        // Close can race the system accepting this request.
                        [self.center removePendingNotificationRequestsWithIdentifiers:@[identifier]];
                        [self.center removeDeliveredNotificationsWithIdentifiers:@[identifier]];
                    } else if (deliveryError) {
                        [self.identifiers removeObjectForKey:key];
                    }
                });
            }];
        });
    }];
}

- (void)close:(uint64_t)notificationId {
    NSNumber *key = @(notificationId);
    NSString *identifier = self.identifiers[key];
    if (!identifier) return;
    [self.identifiers removeObjectForKey:key];
    [self.center removePendingNotificationRequestsWithIdentifiers:@[identifier]];
    [self.center removeDeliveredNotificationsWithIdentifiers:@[identifier]];
}

- (void)dispose {
    self.activate = NULL;
    self.context = NULL;
    if (self.center.delegate == self) self.center.delegate = nil;
    NSArray<NSString *> *identifiers = self.identifiers.allValues;
    [self.identifiers removeAllObjects];
    [self.center removePendingNotificationRequestsWithIdentifiers:identifiers];
    [self.center removeDeliveredNotificationsWithIdentifiers:identifiers];
}

- (BOOL)ownsRequest:(UNNotificationRequest *)request {
    NSNumber *key = request.content.userInfo[@"groveNotification"];
    return [key isKindOfClass:NSNumber.class] &&
        [self.identifiers[key] isEqualToString:request.identifier];
}

- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completion {
    dispatch_async(dispatch_get_main_queue(), ^{
        UNNotificationPresentationOptions options = UNNotificationPresentationOptionNone;
        if ([self ownsRequest:notification.request]) {
            if (@available(macOS 11.0, *)) {
                options = UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList;
            } else {
                options = UNNotificationPresentationOptionAlert;
            }
        }
        completion(options);
    });
}

- (void)userNotificationCenter:(UNUserNotificationCenter *)center
didReceiveNotificationResponse:(UNNotificationResponse *)response
         withCompletionHandler:(void (^)(void))completion {
    dispatch_async(dispatch_get_main_queue(), ^{
        UNNotificationRequest *request = response.notification.request;
        if (self.activate && [response.actionIdentifier isEqualToString:UNNotificationDefaultActionIdentifier] &&
            [self ownsRequest:request]) {
            NSNumber *key = request.content.userInfo[@"groveNotification"];
            uint64_t notificationId = key.unsignedLongLongValue;
            [self close:notificationId];
            self.activate(self.context, notificationId);
        }
        completion();
    });
}
@end

void *grove_notifications_create(const void *context, GroveNotificationActivation activate) {
    // A bare cargo-run executable has no system notification identity.
    if (!NSBundle.mainBundle.bundleIdentifier || ![NSBundle.mainBundle.bundleURL.pathExtension isEqualToString:@"app"]) {
        return NULL;
    }
    GroveNotificationDelegate *delegate = [GroveNotificationDelegate new];
    delegate.center = UNUserNotificationCenter.currentNotificationCenter;
    delegate.identifiers = [NSMutableDictionary new];
    delegate.context = context;
    delegate.activate = activate;
    delegate.center.delegate = delegate;
    return (__bridge_retained void *)delegate;
}

void grove_notifications_show(void *handle, uint64_t notificationId, const uint8_t *title, size_t title_len,
                              const uint8_t *body, size_t body_len) {
    GroveNotificationDelegate *delegate = (__bridge GroveNotificationDelegate *)handle;
    NSString *headline = [[NSString alloc] initWithBytes:title length:title_len encoding:NSUTF8StringEncoding];
    NSString *message = [[NSString alloc] initWithBytes:body length:body_len encoding:NSUTF8StringEncoding];
    if (headline && message) [delegate show:notificationId title:headline body:message];
}

void grove_notifications_close(void *handle, uint64_t notificationId) {
    GroveNotificationDelegate *delegate = (__bridge GroveNotificationDelegate *)handle;
    [delegate close:notificationId];
}

void grove_notifications_dispose(void *handle) {
    GroveNotificationDelegate *delegate = (__bridge_transfer GroveNotificationDelegate *)handle;
    [delegate dispose];
}
