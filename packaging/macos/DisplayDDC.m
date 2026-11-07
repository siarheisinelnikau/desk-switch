// A single, explicitly selected VCP 0x60 transaction. Discovery never sends DDC.
#import <Foundation/Foundation.h>
#import <ApplicationServices/ApplicationServices.h>
#import <IOKit/IOKitLib.h>
#import <IOKit/i2c/IOI2CInterface.h>
#include <mach/mach_time.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#if defined(__arm64__)
#include "i2c.h"
#endif

static BOOL issued = NO;
static void output(NSDictionary *value) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:value options:0 error:nil];
    fwrite(data.bytes, 1, data.length, stdout); fputc('\n', stdout);
}
static int failure(NSString *message) {
    output(@{@"issued":@(issued), @"error":message}); return 0;
}
static CGDirectDisplayID selected(const char *text) {
    NSString *identity = [NSString stringWithUTF8String:text];
    CGDirectDisplayID displays[128]; uint32_t count = 0;
    if (CGGetOnlineDisplayList(128, displays, &count) != kCGErrorSuccess) return 0;
    CGDirectDisplayID selected=0;
    for (uint32_t i = 0; i < count; ++i) {
        CFUUIDRef uuid = CGDisplayCreateUUIDFromDisplayID(displays[i]);
        if (!uuid) continue;
        NSString *name = CFBridgingRelease(CFUUIDCreateString(NULL, uuid)); CFRelease(uuid);
        if ([name caseInsensitiveCompare:identity] == NSOrderedSame && !CGDisplayIsBuiltin(displays[i])) {if (selected) return 0;selected=displays[i];}
    }
    return selected;
}

#if defined(__arm64__)
static IOReturn transaction(CGDirectDisplayID display, BOOL write, uint16_t value, uint8_t reply[12]) {
    DisplayInfos displays[MAX_DISPLAYS] = {};
    CGDisplayCount count = getOnlineDisplayInfos(displays);
    DDCTransport transport = {.service=NULL, .chipAddress=DDC_CHIP_ADDRESS_DEFAULT};
    for (uint32_t i=0; i<count; ++i) if (displays[i].id == display) { transport=getDisplayDDCTransport(&displays[i]); break; }
    if (!transport.service) return kIOReturnUnsupported;
    DDCPacket packet=createDDCPacket(INPUT);
    IOReturn result;
    if (write) {
        prepareDDCWrite(&packet, value);
        issued=YES;
        usleep(DDC_WAIT);
        result=IOAVServiceWriteI2C(transport.service,transport.chipAddress,packet.inputAddr,packet.data,6);
    } else {
        prepareDDCRead(&packet);
        usleep(DDC_WAIT);
        result=IOAVServiceWriteI2C(transport.service,transport.chipAddress,packet.inputAddr,packet.data,4);
        if (result==kIOReturnSuccess) result=performDDCReadAtChipAddress(transport.service,transport.chipAddress,&packet);
        if (result==kIOReturnSuccess) memcpy(reply,packet.data,12);
    }
    CFRelease(transport.service);
    return result;
}
#else
static IOReturn transaction(CGDirectDisplayID display, BOOL write, uint16_t value, uint8_t reply[12]) {
    io_service_t framebuffer=CGDisplayIOServicePort(display);
    if (!framebuffer) return kIOReturnUnsupported;
    IOItemCount count=0;
    IOReturn result=IOFBGetI2CInterfaceCount(framebuffer,&count);
    if (result!=kIOReturnSuccess) return result;
    IOI2CConnectRef connection=NULL; uint32_t replyType=kIOI2CDDCciReplyTransactionType;
    for (IOItemCount bus=0; bus<count && bus<128; ++bus) {
        io_service_t interface=0;
        if (IOFBCopyI2CInterfaceForBus(framebuffer,bus,&interface)!=kIOReturnSuccess) continue;
        uint32_t types=0;
        CFTypeRef property=IORegistryEntryCreateCFProperty(interface,CFSTR(kIOI2CTransactionTypesKey),NULL,0);
        if (property && CFGetTypeID(property)==CFNumberGetTypeID()) CFNumberGetValue(property,kCFNumberSInt32Type,&types);
        if (property) CFRelease(property);
        BOOL usable=(types & (1u<<kIOI2CSimpleTransactionType)) && (write || (types & ((1u<<kIOI2CDDCciReplyTransactionType)|(1u<<kIOI2CSimpleTransactionType))));
        if (usable) result=IOI2CInterfaceOpen(interface,0,&connection);
        IOObjectRelease(interface);
        if (connection) { if (!(types & (1u<<kIOI2CDDCciReplyTransactionType))) replyType=kIOI2CSimpleTransactionType; break; }
    }
    if (!connection) return kIOReturnUnsupported;
    uint8_t bytes[7]={0x51,write?0x84:0x82,write?0x03:0x01,0x60,0,0,0};
    uint32_t length=write?7:5;
    if (write) {bytes[4]=(value>>8)&255;bytes[5]=value&255;}
    uint8_t sum=0x6e;for (uint32_t i=0;i<length-1;++i) sum^=bytes[i];bytes[length-1]=sum;
    IOI2CRequest request={0};
    request.sendAddress=0x6e;request.sendTransactionType=kIOI2CSimpleTransactionType;
    request.sendBuffer=(vm_address_t)bytes;request.sendBytes=length;
    if (!write) {
        mach_timebase_info_data_t scale; mach_timebase_info(&scale);
        request.minReplyDelay=50000000ull*scale.denom/scale.numer;
        request.replyAddress=0x6f;request.replyTransactionType=replyType;
        request.replyBuffer=(vm_address_t)reply;request.replyBytes=11;
    }
    // Never try a second bus after sending this request, even on an error.
    if (write) issued=YES;
    result=IOI2CSendRequest(connection,0,&request);
    if (result==kIOReturnSuccess) result=request.result;
    IOI2CInterfaceClose(connection,0);
    return result;
}
#endif

int main(int argc,const char **argv) {
    @autoreleasepool {
        if (argc==2 && !strcmp(argv[1],"--version")) {puts("Desk Switch DDC helper 1"); return 0;}
        if (argc<3 || argc>4 || (strcmp(argv[1],"read") && strcmp(argv[1],"select"))) return failure(@"Expected read UUID or select UUID INPUT");
        BOOL write=!strcmp(argv[1],"select");
        if (argc!=(write?4:3)) return failure(@"Invalid DDC arguments");
        CGDirectDisplayID display=selected(argv[2]);
        if (!display) return failure(@"The selected external display is unavailable or its identity changed");
        unsigned long input=0;
        if (write) {char *end=NULL;input=strtoul(argv[3],&end,10);if (!end || *end || input==0 || input>65535) return failure(@"Invalid monitor input");}
        uint8_t reply[12]={0};
        IOReturn result=transaction(display,write,(uint16_t)input,reply);
        if (result!=kIOReturnSuccess) return failure([NSString stringWithFormat:@"macOS DDC/CI %@ failed (0x%08x). This GPU, adapter or monitor may not expose a supported DDC transport.",write?@"selection":@"read",result]);
        if (write) {output(@{@"issued":@YES});return 0;}
        uint8_t checksum=0x50;for(int i=0;i<11;++i)checksum^=reply[i];
        if (reply[0]!=0x6e || (reply[1]&0x7f)!=8 || reply[2]!=0x02 || reply[3]!=0 || reply[4]!=0x60 || checksum!=0) return failure(@"The monitor returned an invalid VCP 0x60 reply");
        uint16_t current=((uint16_t)reply[8]<<8)|reply[9];
        if (!current) return failure(@"The monitor did not report an active input");
        output(@{@"issued":@NO,@"value":@(current)});
    }
    return 0;
}
